#!/usr/bin/env python3
"""Generate 36 ASCII frames of O, D, Y rotating independently around Y axis.

Uses only a few minimal ASCII characters ('*' and '.').
O is rendered as a solid glyph; D/Y keep thick/hollow silhouettes.
"""
from __future__ import annotations
import math

WIDTH = 38
HEIGHT = 17

# 11x11 glyphs.
GLYPHS = {
    "O": [
        "  *******  ",
        " ********* ",
        " ***   *** ",
        " ***   *** ",
        " ***   *** ",
        " ***   *** ",
        " ***   *** ",
        " ***   *** ",
        " ***   *** ",
        " ********* ",
        "  *******  ",
    ],
    "D": [
        "***********",
        "***********",
        "***     ***",
        "***      **",
        "***      **",
        "***      **",
        "***      **",
        "***      **",
        "***     ***",
        "***********",
        "***********",
    ],
    "Y": [
        "***     ***",
        "***     ***",
        " ***   *** ",
        "  *** ***  ",
        "   *****   ",
        "    ***    ",
        "    ***    ",
        "    ***    ",
        "    ***    ",
        "    ***    ",
        "    ***    ",
    ],
}

H = len(GLYPHS["O"])
W = len(GLYPHS["O"][0])

# (letter, center_col, speed_mult, direction, phase_offset)
LETTERS = [
    ("O", 6,  1.0,  1.0, 0.0),
    ("D", 19, 1.5, -1.0, 0.0),
    ("Y", 31, 2.0,  1.0, 0.0),
]


def blank_canvas() -> list[list[str]]:
    return [[" " for _ in range(WIDTH)] for _ in range(HEIGHT)]


def in_bounds(r: int, c: int) -> bool:
    return 0 <= r < HEIGHT and 0 <= c < WIDTH


def rotate_y(x: float, y: float, z: float, angle: float) -> tuple[float, float, float]:
    cos_a = math.cos(angle)
    sin_a = math.sin(angle)
    return x * cos_a + z * sin_a, y, -x * sin_a + z * cos_a


def generate_frame(n: int) -> list[list[str]]:
    base_angle = 2 * math.pi * (n - 1) / 36.0
    canvas = blank_canvas()

    for letter, center_col, speed, direction, phase in LETTERS:
        angle = direction * speed * base_angle + phase
        pts = []
        for r, row in enumerate(GLYPHS[letter]):
            for c, ch in enumerate(row):
                if ch == "*":
                    x = c - W // 2
                    y = H // 2 - r
                    rx, ry, rz = rotate_y(x, y, 0.0, angle)
                    pts.append((rx, ry, rz))

        # Draw back-to-front so nearer voxels occlude farther ones.
        pts.sort(key=lambda p: p[2])
        for rx, ry, rz in pts:
            if rz < -2.5:
                continue
            rr = round(HEIGHT // 2 - ry * 0.85)
            cc = round(center_col + rx * 1.0)
            if in_bounds(rr, cc):
                # Near side bright, far side dim.
                canvas[rr][cc] = "*" if rz >= -0.5 else "."

    return canvas


def frame_to_text(canvas: list[list[str]]) -> str:
    return "\n".join("".join(row) for row in canvas) + "\n"


def main():
    import pathlib
    out_dir = pathlib.Path(__file__).parent
    for n in range(1, 37):
        canvas = generate_frame(n)
        text = frame_to_text(canvas)
        (out_dir / f"frame_{n}.txt").write_text(text)
    print("Generated frame_1.txt ... frame_36.txt in", out_dir)


if __name__ == "__main__":
    main()
