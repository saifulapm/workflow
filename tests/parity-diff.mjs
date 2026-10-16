// parity-diff.mjs <a.png> <b.png> -- prints "<differing> <total>": how many
// pixels of two screenshots differ, and the pixels of the larger image. A
// pixel one image has and the other lacks counts as different. Reads 8-bit
// RGB and RGBA PNGs without interlace, which is what the browser writes.
import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";

function decode(path) {
  const buf = readFileSync(path);
  let pos = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const data = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const type = buf.toString("latin1", pos + 4, pos + 8);
    const body = buf.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      const depth = body[8];
      const color = body[9];
      if (depth !== 8 || body[12] !== 0 || (color !== 2 && color !== 6)) {
        throw new Error(`${path}: only 8-bit RGB or RGBA without interlace`);
      }
      channels = color === 6 ? 4 : 3;
    } else if (type === "IDAT") {
      data.push(body);
    }
    pos += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * channels;
  const px = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= channels ? px[y * stride + x - channels] : 0;
      const up = y ? px[(y - 1) * stride + x] : 0;
      const upleft = y && x >= channels ? px[(y - 1) * stride + x - channels] : 0;
      let add = 0;
      if (filter === 1) add = left;
      else if (filter === 2) add = up;
      else if (filter === 3) add = (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - upleft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upleft);
        add = pa <= pb && pa <= pc ? left : pb <= pc ? up : upleft;
      }
      px[y * stride + x] = (line[x] + add) & 255;
    }
  }
  return { width, height, channels, px };
}

const [a, b] = process.argv.slice(2).map(decode);
const w = Math.max(a.width, b.width);
const h = Math.max(a.height, b.height);
let differing = 0;
for (let y = 0; y < h; y++) {
  for (let x = 0; x < w; x++) {
    if (x >= a.width || x >= b.width || y >= a.height || y >= b.height) {
      differing++;
      continue;
    }
    const i = y * a.width * a.channels + x * a.channels;
    const j = y * b.width * b.channels + x * b.channels;
    for (let c = 0; c < 3; c++) {
      if (a.px[i + c] !== b.px[j + c]) {
        differing++;
        break;
      }
    }
  }
}
console.log(differing, w * h);
