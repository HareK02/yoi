// Synthetic raster bytes only; no fixture CSS, DOM renderer, image URL or UI branch.
// A real PNG with 1200×800 natural dimensions and simple colored checker/landscape shapes.
import { deflateSync } from "node:zlib";
export const fixtureImageWidth = 1200;
export const fixtureImageHeight = 800;
function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type: string, data: Uint8Array): Uint8Array {
  const bytes = new Uint8Array(data.length + 12);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, data.length);
  bytes.set(new TextEncoder().encode(type), 4);
  bytes.set(data, 8);
  view.setUint32(data.length + 8, crc32(bytes.subarray(4, data.length + 8)));
  return bytes;
}
const ihdr = new Uint8Array(13);
const dimensions = new DataView(ihdr.buffer);
dimensions.setUint32(0, fixtureImageWidth);
dimensions.setUint32(4, fixtureImageHeight);
ihdr[8] = 8;
ihdr[9] = 2; // 8-bit RGB, no interlacing.
const rowLength = fixtureImageWidth * 3 + 1;
const pixels = new Uint8Array(rowLength * fixtureImageHeight);
for (let y = 0; y < fixtureImageHeight; y++) {
  for (let x = 0; x < fixtureImageWidth; x++) {
    const checker = (Math.floor(x / 100) + Math.floor(y / 100)) % 2;
    let rgb = checker ? [196, 221, 229] : [231, 240, 244];
    if ((x - 930) ** 2 + (y - 170) ** 2 < 105 ** 2) rgb = [238, 175, 72];
    const ridge = 480 - 150 * Math.sin(x / 210);
    if (y > ridge) rgb = checker ? [42, 112, 123] : [52, 135, 141];
    if (y > 650 + 55 * Math.cos(x / 140)) rgb = checker ? [25, 64, 83] : [34, 82, 100];
    pixels.set(rgb, y * rowLength + 1 + x * 3);
  }
}
const parts = [
  new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", ihdr),
  chunk("IDAT", new Uint8Array(deflateSync(pixels, { level: 9 }))),
  chunk("IEND", new Uint8Array()),
];
export const fixturePng = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
let offset = 0;
for (const part of parts) {
  fixturePng.set(part, offset);
  offset += part.length;
}
if (fixturePng.length >= 256 * 1024) throw new Error("Synthetic PNG exceeds raster byte cap");
