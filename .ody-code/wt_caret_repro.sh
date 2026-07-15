#!/usr/bin/env bash
# Reproduce the app's per-frame byte pattern inside a real terminal.
# Faithful to ody-tui try_draw + sync_update:
#   BSU, ClearToEnd sweep (every row w/ trailing blanks, top->bottom),
#   changed-cell Puts (spinner row), DECSCUSR, SHOW, CUP(composer), ESU.
# Usage: wt_caret_repro.sh [frames] [variant]
#   variant "full"   : sweep + puts + cursor ops   (the real pattern)
#   variant "nosweep": puts + cursor ops only
set -u
frames="${1:-250}"
variant="${2:-full}"

read -r rows cols < <(stty size)
status_row=$((rows - 3))   # 1-based "Working" row
composer_row=$((rows - 2)) # 1-based composer row
footer_row=$((rows - 1))   # 1-based footer row... keep one row spare below
design="【 Design 】"
design_cells=12
design_x=$((cols - design_cells - 2 + 1))  # right_aligned_x (1-based), trailing 2 cols
footer_clearend_x=$((cols - 2))            # ClearToEnd x on footer row (0-based 298 -> 1-based 299?)
# diff_buffers uses 0-based x=last_nonblank+1 -> crossterm MoveTo is 0-based too -> printf CUP is 1-based: +1
footer_clearend_cup=$((cols - 2 + 1))

printf '\e[2J\e[H'
# static content drawn once
printf '\e[%d;1H  ◦ Working (7s • esc to interrupt)' "$status_row"
printf '\e[%d;1H› ' "$composer_row"
printf '\e[%d;1H  kimi_ranweiwei/kimi-for-coding · Ask for approval · Context 79%% left · 1.52M used' "$footer_row"
printf '\e[%d;%dH%s' "$footer_row" "$design_x" "$design"

sleep 1

i=0
while [ "$i" -lt "$frames" ]; do
  out='\e[?2026h'
  if [ "$variant" = "full" ]; then
    # ClearToEnd sweep for every row with trailing blanks (top->bottom).
    r=1
    while [ "$r" -le "$rows" ]; do
      if [ "$r" -eq "$status_row" ]; then cx=32
      elif [ "$r" -eq "$composer_row" ]; then cx=3
      elif [ "$r" -eq "$footer_row" ]; then cx=$footer_clearend_cup
      else cx=2
      fi
      out+="\\e[${r};${cx}H\\e[K"
      r=$((r + 1))
    done
  fi
  # spinner put (glyph + truecolor shimmer-ish color change)
  shade=$((128 + (i % 8) * 12))
  out+="\\e[${status_row};3H\\e[38;2;${shade};${shade};${shade}m◦\\e[m"
  # cursor ops exactly as try_draw: DECSCUSR, SHOW, CUP(composer col 3)
  out+='\e[0 q\e[?25h'
  out+="\\e[${composer_row};3H"
  out+='\e[?2026l'
  printf '%b' "$out"
  i=$((i + 1))
  sleep 0.032
done

# leave cursor shown at composer, then hold so we can screenshot the aftermath
printf '\e[?25h\e[%d;3H' "$composer_row"
sleep 5
