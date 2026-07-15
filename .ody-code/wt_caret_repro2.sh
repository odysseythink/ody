#!/usr/bin/env bash
# Phase A: frames ending with SHOW + CUP(composer)   -> expect caret at composer
# Phase B: frames ending with HIDE only (\e[?25l)     -> theory: WT caret parks at repaint end
# Phase C: SHOW once, CUP to marker, then \e[25l (no ?), idle -> does WT hide? (tests \e[25l handling)
# Phase D: same but \e[?25l -> control: WT should hide
set -u
read -r rows cols < <(stty size)
status_row=$((rows - 3))
composer_row=$((rows - 2))
footer_row=$((rows - 1))
design="【 Design 】"
design_x=$((cols - 12 - 2 + 1))
footer_clearend_cup=$((cols - 2 + 1))

draw_static() {
  printf '\e[2J\e[H'
  printf '\e[%d;1H  * Working (4m 03s - esc to interrupt)' "$status_row"
  printf '\e[%d;1H> Run /review on my current changes' "$composer_row"
  printf '\e[%d;1H  kimi_ranweiwei/kimi-for-coding - Ask for approval - Context 79%% left - 1.52M used' "$footer_row"
  printf '\e[%d;%dH%s' "$footer_row" "$design_x" "$design"
}

sweep() {
  local r=1 cx
  while [ "$r" -le "$rows" ]; do
    if [ "$r" -eq "$status_row" ]; then cx=40
    elif [ "$r" -eq "$composer_row" ]; then cx=32
    elif [ "$r" -eq "$footer_row" ]; then cx=$footer_clearend_cup
    else cx=2
    fi
    printf '\e[%d;%dH\e[K' "$r" "$cx"
    r=$((r + 1))
  done
}

banner() { printf '\e[2;2H\e[1;33m%s\e[m' "$1"; }

draw_static
banner "PHASE A: show+cup composer (12s)"
i=0
while [ "$i" -lt 150 ]; do
  sweep
  printf '\e[%d;3H*' "$status_row"
  printf '\e[0 q\e[?25h\e[%d;3H' "$composer_row"
  i=$((i + 1))
  sleep 0.032
done

banner "PHASE B: hide only (12s)"
i=0
while [ "$i" -lt 150 ]; do
  sweep
  printf '\e[%d;3H*' "$status_row"
  printf '\e[?25l'
  i=$((i + 1))
  sleep 0.032
done

banner "PHASE C: show at composer then ESC[25l no-? (10s)"
printf '\e[?25h\e[%d;3H' "$composer_row"
sleep 1
printf '\e[25l'
sleep 9

banner "PHASE D: show at composer then ESC[?25l (10s)"
printf '\e[?25h\e[%d;3H' "$composer_row"
sleep 1
printf '\e[?25l'
sleep 9

banner "DONE"
printf '\e[?25h\e[%d;3H' "$composer_row"
sleep 3
