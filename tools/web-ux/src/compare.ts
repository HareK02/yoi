import { Buffer } from "node:buffer";
import { basename, dirname, join, relative, resolve } from "@std/path";
import { chromium } from "playwright";
import pixelmatch from "pixelmatch";
import { PNG } from "pngjs";
import { ensurePrivateDirectory, makePrivate, sha256File } from "./artifacts.ts";
import type { CaptureEvidence, ReviewContext } from "./types.ts";

export type CompareOptions = {
  before: string;
  after: string;
  outputDirectory: string;
  threshold?: number;
};

type Pair = {
  key: string;
  before: CaptureEvidence;
  after: CaptureEvidence;
  beforePath: string;
  afterPath: string;
  diffPath: string;
  changedPixels: number;
  totalPixels: number;
  dimensionMismatch: boolean;
};

function key(capture: CaptureEvidence): string {
  const viewport = capture.viewport.label ?? `${capture.viewport.width}x${capture.viewport.height}`;
  return [capture.persona.id, capture.route.id, viewport, capture.capturePoint.id].join("/");
}

async function readManifest(path: string): Promise<ReviewContext> {
  const parsed = JSON.parse(await Deno.readTextFile(path));
  if (parsed.schemaVersion !== 1 || !Array.isArray(parsed.captures)) {
    throw new Error(`not a web-ux review context: ${path}`);
  }
  return parsed;
}

function viewportScreenshot(capture: CaptureEvidence): string | null {
  return capture.screenshots.find((item) => item.kind === "viewport")?.bundlePath ??
    capture.screenshots[0]?.bundlePath ?? null;
}

function dataUrl(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return `data:image/png;base64,${btoa(binary)}`;
}

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll(
    '"',
    "&quot;",
  );
}

export async function compare(options: CompareOptions): Promise<string> {
  const beforeManifestPath = resolve(options.before);
  const afterManifestPath = resolve(options.after);
  const [before, after] = await Promise.all([
    readManifest(beforeManifestPath),
    readManifest(afterManifestPath),
  ]);
  const outputDirectory = resolve(options.outputDirectory);
  await ensurePrivateDirectory(outputDirectory);
  const afterByKey = new Map(after.captures.map((capture) => [key(capture), capture]));
  const pairs: Pair[] = [];
  const unmatchedBefore: string[] = [];
  for (const earlier of before.captures) {
    const later = afterByKey.get(key(earlier));
    const earlierScreenshot = viewportScreenshot(earlier);
    const laterScreenshot = later ? viewportScreenshot(later) : null;
    if (!later || !earlierScreenshot || !laterScreenshot) {
      unmatchedBefore.push(key(earlier));
      continue;
    }
    afterByKey.delete(key(earlier));
    const beforePath = resolve(dirname(beforeManifestPath), earlierScreenshot);
    const afterPath = resolve(dirname(afterManifestPath), laterScreenshot);
    const [beforeImage, afterImage] = [
      PNG.sync.read(Buffer.from(Deno.readFileSync(beforePath))),
      PNG.sync.read(Buffer.from(Deno.readFileSync(afterPath))),
    ];
    const dimensionMismatch = beforeImage.width !== afterImage.width ||
      beforeImage.height !== afterImage.height;
    const width = Math.max(beforeImage.width, afterImage.width);
    const height = Math.max(beforeImage.height, afterImage.height);
    const diff = new PNG({ width, height, fill: true });
    let changedPixels = width * height;
    if (!dimensionMismatch) {
      changedPixels = pixelmatch(beforeImage.data, afterImage.data, diff.data, width, height, {
        threshold: options.threshold ?? 0.1,
        includeAA: false,
      });
    }
    const diffPath = join(outputDirectory, "diffs", `${key(earlier).replaceAll("/", "--")}.png`);
    await ensurePrivateDirectory(dirname(diffPath));
    await Deno.writeFile(diffPath, PNG.sync.write(diff), { mode: 0o600 });
    await makePrivate(diffPath);
    pairs.push({
      key: key(earlier),
      before: earlier,
      after: later,
      beforePath,
      afterPath,
      diffPath,
      changedPixels,
      totalPixels: width * height,
      dimensionMismatch,
    });
  }
  const cells: string[] = [];
  for (const pair of pairs) {
    const [beforeBytes, afterBytes, diffBytes] = await Promise.all([
      Deno.readFile(pair.beforePath),
      Deno.readFile(pair.afterPath),
      Deno.readFile(pair.diffPath),
    ]);
    cells.push(
      `<section><h2>${escapeHtml(pair.key)}</h2><p>${
        pair.dimensionMismatch
          ? "dimension mismatch"
          : `${pair.changedPixels} / ${pair.totalPixels} pixels changed`
      }</p><div class="row"><figure><img src="${
        dataUrl(beforeBytes)
      }"><figcaption>before</figcaption></figure><figure><img src="${
        dataUrl(afterBytes)
      }"><figcaption>after</figcaption></figure><figure><img src="${
        dataUrl(diffBytes)
      }"><figcaption>diff</figcaption></figure></div></section>`,
    );
  }
  const html =
    `<!doctype html><meta charset="utf-8"><title>Web UX comparison</title><style>body{margin:0;padding:20px;background:#e8e8e8;color:#111;font:14px system-ui}section{background:white;border:1px solid #aaa;margin:0 0 24px;padding:12px}.row{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px}figure{margin:0}img{width:100%;height:auto;border:1px solid #ddd}figcaption{text-align:center;padding:6px}h2{font-size:16px;margin:0}p{color:#555}</style>${
      cells.join("")
    }`;
  const htmlPath = join(outputDirectory, "comparison.html");
  const pngPath = join(outputDirectory, "comparison.png");
  await Deno.writeTextFile(htmlPath, html, { mode: 0o600 });
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1800, height: 1000 } });
    await page.setContent(html, { waitUntil: "load" });
    await page.screenshot({ path: pngPath, fullPage: true, animations: "disabled" });
    await makePrivate(pngPath);
  } finally {
    await browser.close();
  }
  const report = {
    schemaVersion: 1,
    before: beforeManifestPath,
    after: afterManifestPath,
    createdAt: new Date().toISOString(),
    threshold: options.threshold ?? 0.1,
    pairs: await Promise.all(pairs.map(async (pair) => ({
      key: pair.key,
      changedPixels: pair.changedPixels,
      totalPixels: pair.totalPixels,
      changedRatio: pair.totalPixels === 0 ? 0 : pair.changedPixels / pair.totalPixels,
      dimensionMismatch: pair.dimensionMismatch,
      diff: relative(outputDirectory, pair.diffPath),
      diffSha256: await sha256File(pair.diffPath),
    }))),
    unmatchedBefore,
    unmatchedAfter: [...afterByKey.keys()],
    contactSheet: { html: basename(htmlPath), png: basename(pngPath) },
  };
  const reportPath = join(outputDirectory, "comparison.json");
  await Deno.writeTextFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  return reportPath;
}
