#!/usr/bin/env bash
# Focused: app-style frames that HIDE the cursor (cursor_pos=None path).
# Chain under test: app sends \e[?25l -> ConPTY re-renders -> WT shows what?
# Row 1 is a persistent banner (excluded from sweep).
set -u
read -r rows cols < <(stty size)
status_row=$((rows - 3))
composer_row=$((rows - 2))
footer_row=$((rows - 1))
design="【 Design 】"
design_x=$((cols - 12 - 2 + 1))
footer_clearend_cup=$((cols - 2 + 1))

printf '\e[2J\e[H'
printf '\e[1;2H\e[1;33mHIDE-FRAMES TEST v3\e[m'
printf '\e[%d;1H  * Working (4m 03s - esc to interrupt)' "$status_row"
printf '\e[%d;1H> Run /review on my current changes' "$composer_row"
printf '\e[%d;1H  kimi_ranweiwei/kimi-for-coding - Ask for approval - Context 79%% left - 1.52M used' "$footer_row"
printf '\e[%d;%dH%s' "$footer_row" "$design_x" "$design"
# show cursor at composer first (idle state before the turn)
printf '\e[?25h\e[%d;3H' "$composer_row"
sleep 3

# now frames with hidden cursor (turn running, cursor_pos=None)
i=0
while [ "$i" -lt 600 ]; do
  r=2
  while [ "$r" -le "$rows" ]; do
    if [ "$r" -eq "$status_row" ]; then cx=40
    elif [ "$r" -eq "$composer_row" ]; then cx=32
    elif [ "$r" -eq "$footer_row" ]; then cx=$footer_clearend_cup
    else cx=2
    fi
    printf '\e[%d;%dH\e[K' "$r" "$cx"
    r=$((r + 1))
  done
  printf '\e[%d;3H*' "$status_row"
  printf '\e[?25l'
  i=$((i + 1))
  sleep 0.032
done
printf '\e[1;60H\e[1;32mLOOP DONE, idle, cursor still hidden\e[m'
sleep 20
