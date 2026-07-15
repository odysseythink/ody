const fs = require('fs');
const file = process.argv[2];
const off = parseInt(process.argv[3], 10);
const len = parseInt(process.argv[4], 10);
const buf = fs.readFileSync(file);
const chunk = buf.subarray(off, off + len);
let out = '';
for (const b of chunk) {
  if (b === 0x1b) out += '\\e';
  else if (b === 0x0d) out += '\\r';
  else if (b === 0x0a) out += '\\n';
  else if (b >= 0x20 && b <= 0x7e) out += String.fromCharCode(b);
  else out += '\\x' + b.toString(16).padStart(2, '0');
}
console.log(out);
