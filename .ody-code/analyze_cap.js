#!/usr/bin/env node
// Minimal VT-stream analyzer: tracks cursor position & visibility through a
// captured TUI byte stream and reports per-frame (per ESU) end state, plus
// every write that touches the bottom rows.
const fs = require('fs');

const file = process.argv[2];
const buf = fs.readFileSync(file);

let x = 0, y = 0, visible = true;
const frames = [];        // completed frames (ended by ESU)
let cur = null;           // current frame accumulator
let pendingTitle = [];

function startFrame() { cur = { ops: [], endX: 0, endY: 0, endVisible: true, bytes: 0 }; }
startFrame();

function isWide(cp) {
  return (cp >= 0x1100 && cp <= 0x115F) || (cp >= 0x2E80 && cp <= 0x303E) ||
    (cp >= 0x3041 && cp <= 0x33FF) || (cp >= 0x3400 && cp <= 0x4DBF) ||
    (cp >= 0x4E00 && cp <= 0x9FFF) || (cp >= 0xA000 && cp <= 0xA4CF) ||
    (cp >= 0xAC00 && cp <= 0xD7A3) || (cp >= 0xF900 && cp <= 0xFAFF) ||
    (cp >= 0xFE30 && cp <= 0xFE4F) || (cp >= 0xFF00 && cp <= 0xFF60) ||
    (cp >= 0xFFE0 && cp <= 0xFFE6) || (cp >= 0x20000 && cp <= 0x2FFFD) ||
    (cp >= 0x30000 && cp <= 0x3FFFD);
}

let i = 0;
const N = buf.length;
while (i < N) {
  const b = buf[i];
  if (b === 0x1b) {
    if (buf[i + 1] === 0x5b) { // CSI
      let j = i + 2, params = '';
      while (j < N && !((buf[j] >= 0x40 && buf[j] <= 0x7e))) { params += String.fromCharCode(buf[j]); j++; }
      const final = String.fromCharCode(buf[j] || 0);
      const raw = buf.subarray(i, j + 1).toString('latin1');
      if (final === 'H' || final === 'f') {
        const [r, c] = params.replace('?', '').split(';').map(s => parseInt(s || '1', 10));
        y = (isNaN(r) ? 1 : r) - 1; x = (isNaN(c) ? 1 : c) - 1;
        cur.ops.push(`CUP(${y},${x})`);
      } else if (final === 'K') {
        cur.ops.push(`EL@${y},${x}`);
      } else if (final === 'h' || final === 'l') {
        if (params === '?25') { visible = final === 'h'; cur.ops.push(visible ? 'SHOW' : 'HIDE'); }
        else if (params === '?2026') {
          if (final === 'h') { /* BSU */ cur.ops.push('BSU'); }
          else {
            cur.ops.push('ESU');
            cur.endX = x; cur.endY = y; cur.endVisible = visible;
            frames.push(cur); startFrame();
          }
        }
      } else if (final === 'q') {
        cur.ops.push(`DECSCUSR(${params})`);
      } else if (final === 'G' || final === '`') {
        x = parseInt(params || '1', 10) - 1; cur.ops.push(`CHA(${x})`);
      } else if (final === 'C') { x += parseInt(params || '1', 10); }
      else if (final === 'D') { x -= parseInt(params || '1', 10); }
      else if (final === 'E') { y += parseInt(params || '1', 10); x = 0; }
      else if (final === 'F') { y -= parseInt(params || '1', 10); x = 0; }
      else if (final === 'B') { y += parseInt(params || '1', 10); }
      else if (final === 'A') { y -= parseInt(params || '1', 10); }
      else if (final === 'd') { y = parseInt(params || '1', 10) - 1; }
      else if (final === 'r') { cur.ops.push(`DECSTBM(${params})`); }
      else if (final === 'L') { cur.ops.push(`IL(${params})@${y}`); }
      else if (final === 'M') { cur.ops.push(`DL(${params})@${y}`); }
      else if (final === 'S') { cur.ops.push(`SU(${params})`); }
      else if (final === 'T') { cur.ops.push(`SD(${params})`); }
      i = j + 1; cur.bytes += raw.length; continue;
    } else if (buf[i + 1] === 0x5d) { // OSC: ESC ] ... (BEL | ESC \)
      let j = i + 2;
      while (j < N && buf[j] !== 0x07 && !(buf[j] === 0x1b && buf[j + 1] === 0x5c)) j++;
      const content = buf.subarray(i + 2, j).toString('latin1');
      cur.ops.push(`OSC(${content.slice(0, 40)})`);
      i = (buf[j] === 0x1b) ? j + 2 : j + 1; continue;
    } else if (buf[i + 1] === 0x37) { cur.ops.push('DECSC'); i += 2; continue; }
    else if (buf[i + 1] === 0x38) { cur.ops.push('DECRC'); i += 2; continue; }
    else if (buf[i + 1] === 0x50) { // DCS (sixel!) ESC P ... ESC \
      let j = i + 2;
      while (j < N && !(buf[j] === 0x1b && buf[j + 1] === 0x5c)) j++;
      cur.ops.push(`DCS(${(j - i)} bytes)`);
      i = j + 2; continue;
    }
    cur.ops.push(`ESC${String.fromCharCode(buf[i + 1] || 0)}`); i += 2; continue;
  } else if (b === 0x0d) { x = 0; i++; continue; }
  else if (b === 0x0a) { y++; i++; cur.ops.push(`LF->${y}`); continue; }
  else if (b === 0x08) { x = Math.max(0, x - 1); i++; continue; }
  else if (b >= 0x20) {
    // decode one UTF-8 char
    let len = 1;
    if ((b & 0xe0) === 0xc0) len = 2; else if ((b & 0xf0) === 0xe0) len = 3; else if ((b & 0xf8) === 0xf0) len = 4;
    const ch = buf.subarray(i, i + len).toString('utf8');
    const cp = ch.codePointAt(0) || 0;
    const w = isWide(cp) ? 2 : 1;
    cur.ops.push(`T(${y},${x},"${ch.length > 6 ? ch.slice(0, 6) + '…' : ch}")`);
    x += w; i += len; continue;
  }
  i++;
}
if (cur.ops.length) { cur.endX = x; cur.endY = y; cur.endVisible = visible; frames.push(cur); }

console.log(`total bytes: ${N}, frames (ESU-delimited): ${frames.length}`);
console.log(`final cursor: (${y},${x}) visible=${visible}\n`);

// print the last 40 frames compactly
const tail = frames.slice(-40);
tail.forEach((f, idx) => {
  const gi = frames.length - 40 + idx;
  const ops = f.ops;
  // compress: count text ops, list non-text ops
  const texts = ops.filter(o => o.startsWith('T('));
  const others = ops.filter(o => !o.startsWith('T('));
  const bottom = texts.filter(o => {
    const m = o.match(/^T\((\d+),(\d+)/); return m && parseInt(m[1], 10) >= 65;
  });
  console.log(`--- frame ${gi}: end=(${f.endY},${f.endX}) vis=${f.endVisible} bytes=${f.bytes} texts=${texts.length} bottomRowTexts=${bottom.length}`);
  console.log(`    ops: ${others.join(' ')}`);
  if (bottom.length) console.log(`    bottom: ${bottom.slice(0, 8).join(' ')}`);
});
