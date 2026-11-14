// Compare real generated WASM with output from `cargo run -p config-source --example unicode_parity`.
// deno run --allow-read test/config-source/unicode-native-parity.ts ../../target/t830-evidence/unicode-native.json
import { assertEquals } from "jsr:@std/assert";
import init, {
  analyze_snapshot,
  evaluate_snapshot,
  format_source,
} from "../../src/lib/workspace/config-source/generated/config_source_wasm.js";
await init({
  module_or_path: await Deno.readFile(
    new URL(
      "../../src/lib/workspace/config-source/generated/config_source_wasm_bg.wasm",
      import.meta.url,
    ),
  ),
});
const fixtures = JSON.parse(await Deno.readTextFile(Deno.args[0]));
for (const fixture of fixtures) {
  let evaluation;
  try {
    evaluation = { Ok: evaluate_snapshot(fixture.snapshot, fixture.contract) };
  } catch (error) {
    evaluation = { Err: error };
  }
  assertEquals(
    evaluation,
    fixture.evaluation,
    `${fixture.name}: values and diagnostic kind/path/span/provenance`,
  );
  assertEquals(
    analyze_snapshot(fixture.snapshot, "main.dcdl"),
    fixture.analysis,
    `${fixture.name}: language service`,
  );
  let formatted;
  try {
    formatted = {
      Ok: format_source(fixture.snapshot.entries["main.dcdl"].content),
    };
  } catch (error) {
    formatted = { Err: error };
  }
  assertEquals(formatted, fixture.formatted, `${fixture.name}: formatter`);
}
console.log(
  `native / generated WASM: ${fixtures.length} Unicode cases match (evaluation, diagnostics, language service, formatter)`,
);
