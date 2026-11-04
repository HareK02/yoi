// Optional execution adapter: only Deno's built-in WASM runtime, no JS packages.
// Native contracts.rs checks the same golden digests against the native model.
const wasmPath = Deno.args[0];
if (!wasmPath) {
  throw new Error("usage: deno run --allow-read tests/wasm_parity.ts <wasm>");
}
const bytes = await Deno.readFile(wasmPath);
const module = await WebAssembly.compile(bytes);
if (WebAssembly.Module.imports(module).length !== 0) {
  throw new Error("raw parity module must have no host imports");
}
const instance = await WebAssembly.instantiate(module, {});
const length = instance.exports.parity_len as () => number;
const word = instance.exports.parity_word as (index: number) => number;
const fixture = await Deno.readTextFile(
  new URL("fixtures/v1.tsv", import.meta.url),
);
const expected = fixture.trim().split("\n").filter((line) =>
  !line.startsWith("#")
)
  .flatMap((line) =>
    line.split("\t")[2].match(/.{8}/g)!.map((hex) => parseInt(hex, 16))
  );
if (length() !== expected.length) {
  throw new Error("fixture digest count differs");
}
for (let index = 0; index < expected.length; index++) {
  if ((word(index) >>> 0) !== expected[index]) {
    throw new Error(`WASM digest word mismatch at ${index}`);
  }
}
if ((word(expected.length) >>> 0) !== 0xffffffff) {
  throw new Error("out-of-range word did not return sentinel");
}
console.log(
  `WASM canonical SHA256 parity: ${expected.length / 8} fixture objects`,
);
