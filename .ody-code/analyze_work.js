// Analyze work_cap.bin: frame structure around ESU boundaries + timing.
const fs = require('fs');
const bin = fs.readFileSync('E:/ody-rs/.ody-code/work_cap.bin');
const times = fs.readFileSync('E:/ody-rs/.ody-code/work_cap.bin.times', 'utf8')
  .trim().split('\n').map(l => { const [off, len, ts] = l.split(' ').map(Number); return { off, len, ts }; });

function tsOf(byteOff) {
  // binary search: last chunk with off <= byteOff
  let lo = 0, hi = times.length - 1, ans = 0;
  while (lo <= hi) { const mid = (lo + hi) >> 1; if (times[mid].off <= byteOff) { ans = mid; lo = mid + 1; } else hi = mid - 1; }
  return { ts: times[ans].ts, chunk: ans, chunkOff: times[ans].off, chunkLen: times[ans].len };
}

const ESU = Buffer.from('\x1b[?2026l');
const BSU = Buffer.from('\x1b[?2026h');
const CUP_RE = /\x1b\[(\d+);(\d+)H/g;

// find all ESU positions
const esus = [];
let i = 0;
while ((i = bin.indexOf(ESU, i)) !== -1) { esus.push(i); i += 1; }
console.log('total bytes', bin.length, 'ESU count', esus.length);

// For Working-era frames (after first "Working" appearance at 80843), analyze each ESU:
// - bytes until next CUP after ESU
// - CUP target row/col
// - same read chunk? time gap?
let rows = [];
for (const e of esus) {
  if (e < 90000) continue; // only Working era
  CUP_RE.lastIndex = 0;
  const m = CUP_RE.exec(bin.slice(e, e + 400).toString('latin1'));
  if (!m) continue;
  const cupAbs = e + m.index;
  const cupRow = m[1], cupCol = m[2];
  const tE = tsOf(e), tC = tsOf(cupAbs);
  rows.push({
    esu: e, cupAbs, dist: cupAbs - e, cupRow, cupCol,
    esuTs: tE.ts, cupTs: tC.ts, gapMs: tC.ts - tE.ts,
    sameChunk: tE.chunk === tC.chunk,
  });
}
console.log('working-era ESU frames:', rows.length);
// summarize
const gaps = rows.map(r => r.gapMs);
const same = rows.filter(r => r.sameChunk).length;
console.log('same-chunk ESU..CUP:', same, '/', rows.length);
console.log('gap ms: min', Math.min(...gaps), 'max', Math.max(...gaps),
  'avg', (gaps.reduce((a, b) => a + b, 0) / gaps.length).toFixed(2));
// CUP target distribution
const targets = {};
for (const r of rows) targets[`${r.cupRow};${r.cupCol}`] = (targets[`${r.cupRow};${r.cupCol}`] || 0) + 1;
console.log('CUP targets:', targets);
// print first 25 frames
for (const r of rows.slice(0, 25)) {
  console.log(`ESU@${r.esu} ts=${r.esuTs}  CUP->${r.cupRow};${r.cupCol} dist=${r.dist} gap=${r.gapMs}ms sameChunk=${r.sameChunk}`);
}
// what's between ESU and CUP in a sample frame?
if (rows.length > 2) {
  const r = rows[2];
  const seg = bin.slice(r.esu, r.cupAbs + 8).toString('latin1').replace(/\x1b/g, '<ESC>');
  console.log('sample ESU..CUP segment:', JSON.stringify(seg));
}
// Also: what is the last text-batch content before each ESU? find the last 60 printable bytes before ESU
for (const r of rows.slice(0, 8)) {
  const pre = bin.slice(Math.max(0, r.esu - 80), r.esu + 6).toString('latin1').replace(/\x1b/g, '<E>').replace(/[\x00-\x08\x0b-\x1f]+/g, '.');
  console.log(`frame@${r.esu} pre:`, JSON.stringify(pre));
}
