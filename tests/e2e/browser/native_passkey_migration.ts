/// <reference lib="dom" />
// 実Backend + 本番static shell + Chromium native WebAuthn/CTAP2の旧鍵移行matrix。
// navigator.credentials、HTTP応答、暗号検証を置換しない。CDPは合成認証器の
// 能力/本人確認/タッチだけを制御し、credentialのexport/import/deleteはしない。
// 旧鍵fixtureだけは実optionsのuser.idとresident指定を変更してnative createする。
// 登録attestationはuser handleを署名しないため、本番verifier/storeで保存できる。
// 現行製品の登録動作の再現ではなく、旧random handle保存状態の明示的な合成である。
// 実スマホ/OSのprovider選択/物理鍵は対象外。mobileはChromiumのviewportのみ。
import { join } from "@std/path";
import { type Browser, type CDPSession, chromium, type Page } from "playwright";
import { serveProductionShell } from "./static_shell.ts";

const [backend, origin, build, handlePrefix, artifacts] = Deno.args;
if (!backend || !origin || !build || !handlePrefix || !artifacts) {
  throw new Error(
    "Expected owned Backend, origin, production build, synthetic handle prefix, artifacts",
  );
}
const started = performance.now();
const server = await serveProductionShell(backend, origin, build);
let browser: Browser | undefined;
let phase = "launch";
const rows: {
  resident: boolean;
  authenticator: string;
  outcome: string;
  elapsed_ms: number;
}[] = [];
class NativeCheckError extends Error {}
function check(condition: unknown, diagnostic: string): asserts condition {
  // 値をdumpするassertEqualsは使わない。例外/HTTP本文/credential/cookieをログに残さない。
  if (!condition) throw new NativeCheckError(diagnostic);
}
type User = { user_id: string; account_id: string; handle: string };
type Key = {
  key_reference: string;
  normal_login_verified_at: string | null;
  last_used_at: string | null;
};
async function whoami(page: Page) {
  const response = await page.request.get(`${origin}/api/auth/whoami`);
  check(response.status() === 200, "whoami status");
  return await response.json() as {
    actor: User | null;
    authentication_reference: string | null;
    session: { kind: string };
  };
}
async function owner(page: Page, user: User, reference?: string) {
  const current = await whoami(page);
  check(
    current.session.kind === "authenticated" &&
      current.actor?.user_id === user.user_id &&
      current.actor.account_id === user.account_id &&
      current.actor.handle === user.handle,
    "authenticated owner mismatch",
  );
  check(
    typeof current.authentication_reference === "string",
    "missing authentication reference",
  );
  if (reference) {
    check(
      current.authentication_reference === reference,
      "issued authentication mismatch",
    );
  }
  return current;
}
async function keys(page: Page, user: User): Promise<Key[]> {
  const response = await page.request.get(`${origin}/api/auth/passkeys`);
  check(response.status() === 200, "key list status");
  const result = await response.json();
  check(
    result.user.user_id === user.user_id &&
      result.user.account_id === user.account_id,
    "key list owner",
  );
  return result.keys;
}
async function presence(
  cdp: CDPSession,
  authenticatorId: string,
  enabled: boolean,
) {
  await cdp.send("WebAuthn.setAutomaticPresenceSimulation", {
    authenticatorId,
    enabled,
  });
}
async function authenticator(
  cdp: CDPSession,
  resident: boolean,
  automaticPresenceSimulation = true,
) {
  return (await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "usb",
      hasResidentKey: resident,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation,
    },
  })).authenticatorId;
}
async function account(page: Page) {
  await page.goto(origin + "/account");
  await page.getByRole("heading", { name: "Account", exact: true }).waitFor();
  await page.getByRole("button", { name: "Log out", exact: true }).waitFor();
}
async function logout(page: Page) {
  await account(page);
  await page.getByRole("button", { name: "Log out", exact: true }).click();
  await page.waitForURL((url) => url.origin === origin && url.pathname === "/");
  check(
    (await whoami(page)).session.kind === "anonymous",
    "logout must clear authentication",
  );
}
async function login(page: Page, user: User, legacy: boolean) {
  await page.goto(origin + "/login?returnTo=%2Faccount");
  const button = page.getByRole("button", {
    name: legacy ? "Log in with older key" : "Log in with passkey",
    exact: true,
  });
  if (legacy) {
    await page.getByText("Use an older key with account ID", { exact: true })
      .click();
    await page.getByLabel("Account ID (handle)", { exact: true }).fill(
      user.handle,
    );
  } else {
    // 互換入口は閉じたまま。通常loginのrequestにhandle/user/accountが入らないことも検査する。
    check(
      !await page.getByLabel("Account ID (handle)", { exact: true })
        .isVisible(),
      "normal login asks for ID",
    );
  }
  const options = page.waitForResponse((r) =>
    new URL(r.url()).pathname === "/api/auth/passkeys/login/options"
  );
  const complete = page.waitForResponse((r) =>
    new URL(r.url()).pathname === "/api/auth/passkeys/login/complete"
  );
  await button.click();
  const optionResponse = await options;
  check(optionResponse.status() === 200, "login options status");
  const input = optionResponse.request().postDataJSON();
  if (!legacy) {
    check(
      Object.keys(input).every((key) => key === "browser_origin"),
      "normal login selected an account",
    );
    const wire = await optionResponse.json();
    check(
      !wire.public_key.publicKey.allowCredentials?.length,
      "normal login sends an allow-list",
    );
  }
  const response = await complete;
  check(response.status() === 200, "native login complete status");
  const issued = await response.json();
  check(
    issued.user.user_id === user.user_id &&
      issued.user.account_id === user.account_id,
    "login returned wrong owner",
  );
  await page.waitForURL((url) => url.pathname === "/account");
  await owner(page, user, issued.authentication_reference);
}

const terminate = () => {
  void (async () => {
    await browser?.close();
    await server.shutdown();
    Deno.exit(143);
  })();
};
Deno.addSignalListener("SIGTERM", terminate);
try {
  browser = await chromium.launch({ headless: true, timeout: 15_000 });
  for (const resident of [false, true]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 900 },
      serviceWorkers: "block",
    });
    context.setDefaultTimeout(12_000);
    // 製品もfixtureも外部通信しない。loopback proxy以外へのbrowser requestは拒否する。
    await context.route(
      "**/*",
      (route) =>
        new URL(route.request().url()).origin === origin
          ? route.continue()
          : route.abort(),
    );
    let page = await context.newPage();
    let pageErrors = 0;
    page.on("pageerror", () => ++pageErrors);
    let cdp = await context.newCDPSession(page);
    await cdp.send("WebAuthn.enable", { enableUI: false });
    // 両行とも認証器はresident key対応にする。residentは旧credentialの作成指定だけを変え、
    // resident必須の追加が能力不足で拒否されるケースとduplicate exclusionを混同しない。
    const old = await authenticator(cdp, true);
    let completionPosts = 0;
    const completions = () => completionPosts;
    page.on("request", (r) => {
      if (
        new URL(r.url()).pathname === "/api/auth/passkeys/addition/complete"
      ) ++completionPosts;
    });
    try {
      phase = `legacy-fixture-resident-${resident}`;
      await page.goto(origin + "/register");
      // 生options/credentialはpage内にとどめる。user.id/residency以外はServerの要求を維持する。
      const fixture = await page.evaluate(async ({ handle, resident }) => {
        const post = async (path: string, body: unknown) => {
          const response = await fetch(path, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
          });
          if (!response.ok) {
            throw new Error("synthetic legacy fixture HTTP rejected");
          }
          return await response.json();
        };
        const wire = await post("/api/auth/passkeys/registration/options", {
          handle,
          display_name: "Synthetic native migration",
          browser_origin: location.origin,
        });
        const bytes = (value: string) =>
          Uint8Array.from(
            atob(value.replaceAll("-", "+").replaceAll("_", "/")),
            (c) => c.charCodeAt(0),
          );
        const options = wire.public_key.publicKey;
        const random = crypto.getRandomValues(new Uint8Array(32));
        const stable = bytes(options.user.id);
        const differs = stable.length !== random.length ||
          stable.some((byte: number, index: number) => byte !== random[index]);
        options.challenge = bytes(options.challenge);
        options.user.id = random;
        options.excludeCredentials = options.excludeCredentials?.map((
          item: { id: string },
        ) => ({ ...item, id: bytes(item.id) }));
        options.authenticatorSelection = {
          ...options.authenticatorSelection,
          residentKey: resident ? "required" : "discouraged",
          requireResidentKey: resident,
        };
        const credential = await navigator.credentials.create({
          publicKey: options,
        }) as PublicKeyCredential | null;
        if (
          !credential ||
          !(credential.response instanceof AuthenticatorAttestationResponse)
        ) throw new Error("native attestation missing");
        const encode = (value: ArrayBuffer) =>
          btoa(String.fromCharCode(...new Uint8Array(value))).replaceAll(
            "+",
            "-",
          ).replaceAll("/", "_").replaceAll("=", "");
        const issued = await post("/api/auth/passkeys/registration/complete", {
          challenge_id: wire.challenge_id,
          credential: {
            id: credential.id,
            rawId: encode(credential.rawId),
            type: credential.type,
            response: {
              attestationObject: encode(credential.response.attestationObject),
              clientDataJSON: encode(credential.response.clientDataJSON),
              transports: credential.response.getTransports(),
            },
            extensions: credential.getClientExtensionResults(),
          },
        });
        return {
          user: issued.user,
          authentication_reference: issued.authentication_reference,
          randomDiffersFromStable: differs,
        };
      }, {
        handle: `${handlePrefix}-${resident ? "resident" : "nonresident"}`,
        resident,
      });
      check(
        fixture.randomDiffersFromStable,
        "fixture must use a legacy random handle",
      );
      const user: User = fixture.user;
      await owner(page, user, fixture.authentication_reference);
      const initial = await keys(page, user);
      check(
        initial.length === 1 && initial[0].normal_login_verified_at === null,
        "legacy key initially unverified",
      );
      const oldReference = initial[0].key_reference;
      await logout(page);
      phase = `older-key-login-before-resident-${resident}`;
      await login(page, user, true);

      phase = `same-authenticator-excluded-resident-${resident}`;
      const sameStarted = performance.now();
      const add = () =>
        page.getByRole("button", {
          name: `Add passkey to ${user.handle}`,
          exact: true,
        });
      const optionsResponse = page.waitForResponse((r) =>
        new URL(r.url()).pathname === "/api/auth/passkeys/addition/options"
      );
      await add().click();
      check(
        (await optionsResponse).status() === 200,
        "addition options status",
      );
      const excluded = page.getByRole("alert").filter({
        hasText: "[authenticator/excluded]",
      });
      await excluded.waitFor({ state: "visible" });
      check(
        (await excluded.innerText()).includes(
          "different device, security key, or passkey provider",
        ),
        "exclusion needs a concrete alternative",
      );
      check(
        (await excluded.innerText()).includes("do not delete"),
        "exclusion must preserve working access",
      );
      check(
        completions() === 0,
        "native exclusion must not submit completion",
      );
      const afterExcluded = await keys(page, user);
      check(
        afterExcluded.length === 1 &&
          afterExcluded[0].key_reference === oldReference,
        "exclusion changed old key",
      );
      await owner(page, user);
      rows.push({
        resident,
        authenticator: "same",
        outcome: "native-exclusion-no-completion-old-key-preserved",
        elapsed_ms: Math.round(performance.now() - sameStarted),
      });

      phase = `other-authenticator-pending-resident-${resident}`;
      const otherStarted = performance.now();
      // CDPの仮想認証器選択は物理providerの選択UIを再現しない。別targetに別認証器を置き、
      // 旧targetと旧鍵は生存させる。共有contextの既存sessionを使い、cookie/鍵の移送はしない。
      const legacyPage = page;
      const legacyCdp = cdp;
      page = await context.newPage();
      page.on("pageerror", () => ++pageErrors);
      page.on("request", (r) => {
        if (
          new URL(r.url()).pathname === "/api/auth/passkeys/addition/complete"
        ) ++completionPosts;
      });
      cdp = await context.newCDPSession(page);
      await cdp.send("WebAuthn.enable", { enableUI: false });
      const newer = await authenticator(cdp, true, false);
      await account(page);
      // options応答後、native操作はCTAPのpresence=falseで止める。timeout/sleepでpromptを推測しない。
      const beforeAddition = await owner(page, user);
      const additionOptions = page.waitForResponse((r) =>
        new URL(r.url()).pathname === "/api/auth/passkeys/addition/options"
      );
      const additionComplete = page.waitForResponse((r) =>
        new URL(r.url()).pathname === "/api/auth/passkeys/addition/complete"
      );
      await add().click();
      const optionResponse = await additionOptions;
      check(
        optionResponse.status() === 200,
        "other authenticator options status",
      );
      let optionsInput = optionResponse.request().postDataJSON();
      check(
        optionsInput.expected_user_id === user.user_id &&
          optionsInput.expected_account_id === user.account_id &&
          optionsInput.expected_authentication_reference ===
            beforeAddition.authentication_reference,
        "addition must fence displayed owner and session",
      );
      let wire = await optionResponse.json();
      check(
        wire.public_key.publicKey.excludeCredentials.length === 1,
        "old credential must remain excluded",
      );
      check(
        wire.public_key.publicKey.authenticatorSelection.userVerification ===
          "required",
        "addition must require UV",
      );
      check(
        wire.public_key.publicKey.authenticatorSelection.residentKey ===
          "required",
        "product addition must require residency",
      );
      const pending = page.getByRole("status").filter({
        hasText: "Waiting for passkey addition",
      });
      await pending.waitFor({ state: "visible" });
      check(
        await page.getByRole("heading", { name: "Account", exact: true })
          .isVisible(),
        "account hidden while native pending",
      );
      check(
        await add().isDisabled() && completions() === 0,
        "pending must prevent duplicate/early completion",
      );
      for (
        const viewport of [{ width: 1280, height: 900 }, {
          width: 390,
          height: 844,
        }]
      ) {
        await page.setViewportSize(viewport);
        await pending.scrollIntoViewIfNeeded();
        check(
          await pending.isVisible(),
          "pending must remain visible at viewport",
        );
        await page.screenshot({
          path: join(
            artifacts,
            `native-addition-pending-${resident}-${viewport.width}.png`,
          ),
          fullPage: false,
        });
      }
      await page.setViewportSize({ width: 1280, height: 900 });
      // presence=falseで既に待機中のCTAP要求を後からtrueにして解放できるとは仮定しない。
      // pending/取消と成功を分ける。送信前の停止を確認し、別ceremonyを明示開始する。
      await page.getByRole("button", { name: "Stop waiting", exact: true })
        .click();
      await page.getByRole("alert").filter({
        hasText: "Completion was not sent.",
      }).waitFor();
      check(
        completions() === 0 && (await keys(page, user)).length === 1,
        "cancel before touch must not save or complete",
      );
      await presence(cdp, newer, true);
      const freshOptions = page.waitForResponse((r) =>
        new URL(r.url()).pathname === "/api/auth/passkeys/addition/options"
      );
      await add().click();
      const freshResponse = await freshOptions;
      check(freshResponse.status() === 200, "fresh addition options status");
      optionsInput = freshResponse.request().postDataJSON();
      wire = await freshResponse.json();
      phase = `other-authenticator-completion-resident-${resident}`;
      const complete = await additionComplete;
      check(complete.status() === 200, "native addition complete status");
      const issued = await complete.json();
      check(
        issued.user.user_id === user.user_id &&
          issued.user.account_id === user.account_id &&
          typeof issued.authentication_reference === "string",
        "addition complete owner/reference",
      );
      await page.getByRole("status").filter({
        hasText: "Passkey added to your account",
      }).waitFor({ state: "visible" });
      await owner(page, user, issued.authentication_reference);
      check(completions() === 1, "addition must submit exactly once");
      // 元のbinding referenceはreceipt照合用。Cookie発行後も同じ現在ownerから確認する。
      const receipt = await page.request.post(
        `${origin}/api/auth/passkeys/addition/result`,
        {
          headers: { Origin: origin },
          data: {
            challenge_id: wire.challenge_id,
            expected_user_id: user.user_id,
            expected_account_id: user.account_id,
            expected_authentication_reference:
              optionsInput.expected_authentication_reference,
          },
        },
      );
      check(
        receipt.status() === 200,
        "saved addition receipt after Cookie issuance",
      );
      const saved = await receipt.json();
      check(
        saved.status === "saved" && saved.user.user_id === user.user_id &&
          saved.user.account_id === user.account_id,
        "receipt must identify saved owner",
      );
      const added = await keys(page, user);
      check(
        added.length === 2 &&
          added.some((key) => key.key_reference === oldReference),
        "addition must retain older key",
      );
      const newKey = added.find((key) => key.key_reference !== oldReference);
      check(
        newKey && saved.key.key_reference === newKey.key_reference &&
          newKey.normal_login_verified_at === null,
        "storage must not assert normal login",
      );
      await logout(page);
      phase = `handle-free-login-resident-${resident}`;
      await login(page, user, false);
      const verified = await keys(page, user);
      check(
        verified.length === 2 &&
          verified.find((key) => key.key_reference === newKey.key_reference)
              ?.normal_login_verified_at != null,
        "actual handle-free verifier success must mark the new key",
      );
      check(
        verified.find((key) => key.key_reference === oldReference)
          ?.normal_login_verified_at === null,
        "normal login must not mark the unused legacy key",
      );
      await logout(page);
      phase = `older-key-login-after-resident-${resident}`;
      await presence(cdp, newer, false);
      page = legacyPage;
      cdp = legacyCdp;
      await presence(cdp, old, true);
      await login(page, user, true);
      const retained = await keys(page, user);
      check(
        retained.length === 2 &&
          retained.find((key) => key.key_reference === oldReference)
              ?.last_used_at != null,
        "older key must remain usable after migration",
      );
      check(
        retained.find((key) => key.key_reference === newKey.key_reference)
          ?.normal_login_verified_at != null,
        "older login must preserve new key verification",
      );
      check(pageErrors === 0, "product page emitted an uncaught error");
      rows.push({
        resident,
        authenticator: "other",
        outcome:
          "saved-correct-owner-handle-free-verified-older-login-preserved",
        elapsed_ms: Math.round(performance.now() - otherStarted),
      });
    } finally {
      await context.close();
    }
  }
  await Deno.writeTextFile(
    join(artifacts, "native-migration-result.json"),
    JSON.stringify(
      {
        status: "passed",
        boundary:
          "real Backend/static shell/native Chromium CTAP2; synthetic legacy creation override only",
        chromium: browser.version(),
        rows,
        elapsed_ms: Math.round(performance.now() - started),
        viewports: ["1280x900", "390x844"],
        physical_smartphone: "unconfirmed",
        physical_authenticator: "unconfirmed",
      },
      null,
      2,
    ),
  );
  console.error(
    `native-migration: PASS ${rows.length} matrix rows; ${
      Math.round(performance.now() - started)
    }ms`,
  );
} catch (error) {
  const diagnostic = error instanceof NativeCheckError
    ? error.message
    : "browser-or-tool-operation-failed";
  await Deno.writeTextFile(
    join(artifacts, "native-migration-result.json"),
    JSON.stringify(
      {
        status: "failed",
        phase,
        diagnostic,
        rows,
        elapsed_ms: Math.round(performance.now() - started),
        physical_smartphone: "unconfirmed",
        physical_authenticator: "unconfirmed",
      },
      null,
      2,
    ),
  );
  // Playwrightの例外にはrequest/引数を含み得るため本文やstackを保存しない。
  console.error(
    `native-migration: FAIL phase=${phase}; completed_rows=${rows.length}; elapsed_ms=${
      Math.round(performance.now() - started)
    }`,
  );
  Deno.exitCode = 1;
} finally {
  Deno.removeSignalListener("SIGTERM", terminate);
  await browser?.close();
  await server.shutdown();
}
