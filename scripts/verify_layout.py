#!/usr/bin/env python3
"""
verify_layout.py — 验证 figo state 图输出的几何约束

用法:
    echo '{...json...}' | figo.exe state > output.txt
    python3 verify_layout.py output.txt [--json input.json] [--figo path/to/figo]

验证规则:
    1. box 不重叠——任意两个 box 的 rect 不相交
    2. 竖腿对齐——跨层 transition 的竖腿连续或在走廊行连接
    3. 走廊连续——走廊 --- 从 left_x 连续到 right_x 无断裂
    4. label 不换行——label 文本作为连续字符串出现
    5. 走廊两端 + ——走廊与竖腿交汇处有 +
    6. 箭头指向目标——v/▼ 在 to box 中心列上方
"""

import sys
import re
import json
import subprocess
import argparse
from dataclasses import dataclass
from typing import List, Optional, Tuple


@dataclass
class Box:
    """A state box parsed from the output."""
    x: int
    y: int
    w: int
    h: int
    label: str = ""

    @property
    def cx(self) -> int:
        return self.x + self.w // 2

    @property
    def right(self) -> int:
        return self.x + self.w

    @property
    def bottom(self) -> int:
        return self.y + self.h


@dataclass
class CorridorSegment:
    """A horizontal corridor segment on a single row."""
    x1: int  # left end
    x2: int  # right end
    y: int   # row
    label: str = ""  # label text if embedded, "" if just ---


@dataclass
class ValidationResult:
    passed: int = 0
    failed: int = 0
    errors: list = None

    def __post_init__(self):
        if self.errors is None:
            self.errors = []

    def check(self, condition: bool, msg: str):
        if condition:
            self.passed += 1
        else:
            self.failed += 1
            self.errors.append(msg)

    def summary(self) -> str:
        total = self.passed + self.failed
        status = "PASS" if self.failed == 0 else "FAIL"
        lines = [f"Result: {status} ({self.passed}/{total} checks passed)"]
        if self.errors:
            lines.append("Failures:")
            for e in self.errors:
                lines.append(f"  - {e}")
        return "\n".join(lines)


def parse_output(text: str) -> Tuple[List[Box], List[CorridorSegment], List[str]]:
    """Parse figo output text into boxes, corridor segments, and raw lines."""
    lines = text.split("\n")
    boxes = []
    corridors = []

    # Parse boxes: find top border lines like +---...---+
    # Borders may contain v/^ (arrow indicators within the border).
    # Each +...+ on a line is a potential box top, but a border already
    # used as a bottom should not be reused as a top.
    consumed_rows = set()  # (row, x) pairs already used as box borders
    i = 0
    while i < len(lines):
        line = lines[i]
        # Find ALL +...+ patterns on this line (multiple boxes per row)
        for m in re.finditer(r'\+([^\s+]+)\+', line):
            box_x = m.start()
            if (i, box_x) in consumed_rows:
                continue  # already used as a bottom border
            border_w = m.end() - m.start()
            box_y = i
            box_w = border_w
            # Find matching bottom border: first line at same x starting with +
            # with matching width, within a few rows.
            box_h = 0
            for j in range(i + 1, min(i + 6, len(lines))):  # max 5 rows deep
                bottom_line = lines[j]
                # Bottom border must start with + at same x
                if box_x < len(bottom_line) and bottom_line[box_x] == '+':
                    # Check it's a border (next chars are - or v/^)
                    border_end = box_x + box_w - 1
                    if border_end < len(bottom_line) and bottom_line[border_end] == '+':
                        box_h = j - i + 1
                        # Extract label from content line(s)
                        # Only take text within the box's x range
                        content_lines = lines[i+1:j]
                        label_text = ""
                        for cl in content_lines:
                            if box_x + 1 < len(cl):
                                # Extract text between the | markers
                                inner = cl[box_x+1:box_x+box_w-1]
                                stripped = inner.strip()
                                if stripped and stripped not in ('', '|') and not all(c in '|-+v^' for c in stripped):
                                    label_text = stripped
                                    break
                        boxes.append(Box(box_x, box_y, box_w, box_h, label_text))
                        consumed_rows.add((i, box_x))  # top border
                        consumed_rows.add((j, box_x))  # bottom border
                        break
        i += 1

    # Parse corridors: find rows with --- or label text between boxes
    for y, line in enumerate(lines):
        # Find runs of --- (horizontal corridor)
        for m in re.finditer(r'[-]{2,}', line):
            x1 = m.start()
            x2 = m.end() - 1
            # Check if this is a corridor (not a box border)
            if line[x1:x1+1] == '-':
                corridors.append(CorridorSegment(x1, x2, y))

    # Also parse labeled corridors: text between + markers
    for y, line in enumerate(lines):
        # Find segments like +label+ or -label- or --label--
        for m in re.finditer(r'[+-]([^-+]{2,})[+-]', line):
            label = m.group(1).strip()
            if label and not label.isspace():
                corridors.append(CorridorSegment(m.start(), m.end()-1, y, label))

    return boxes, corridors, lines


def verify_boxes_no_overlap(boxes: List[Box], result: ValidationResult):
    """Rule 1: No two boxes overlap (except accepting-state double border)."""
    for i in range(len(boxes)):
        for j in range(i + 1, len(boxes)):
            a, b = boxes[i], boxes[j]
            overlap = not (a.right <= b.x or b.right <= a.x or a.bottom <= b.y or b.bottom <= a.y)
            # Skip if one box is nested inside the other (accepting state double border)
            nested = (a.x <= b.x and a.right >= b.right and a.y <= b.y and a.bottom >= b.bottom) or \
                      (b.x <= a.x and b.right >= a.right and b.y <= a.y and b.bottom >= a.bottom)
            result.check(not overlap or nested,
                f"Boxes overlap: '{a.label}'({a.x},{a.y},{a.w}x{a.h}) vs "
                f"'{b.label}'({b.x},{b.y},{b.w}x{b.h})")


def verify_labels_not_wrapped(lines: List[str], labels: List[str], result: ValidationResult):
    """Rule 4: Each label appears as a contiguous substring."""
    full_text = "\n".join(lines)
    for label in labels:
        if not label:
            continue
        result.check(label in full_text,
            f"Label '{label}' not found as contiguous string (likely wrapped)")


def verify_box_spacing(boxes: List[Box], result: ValidationResult, min_gap: int = 2):
    """Rule: Same-layer boxes have at least min_gap columns between them."""
    # Group boxes by y (same layer)
    by_y = {}
    for b in boxes:
        by_y.setdefault(b.y, []).append(b)

    for y, layer_boxes in by_y.items():
        layer_boxes.sort(key=lambda b: b.x)
        for i in range(len(layer_boxes) - 1):
            gap = layer_boxes[i+1].x - layer_boxes[i].right
            result.check(gap >= min_gap,
                f"Boxes too close at y={y}: '{layer_boxes[i].label}' right={layer_boxes[i].right} "
                f"vs '{layer_boxes[i+1].label}' left={layer_boxes[i+1].x}, gap={gap} < {min_gap}")


def verify_vertical_legs(boxes: List[Box], lines: List[str], result: ValidationResult):
    """Rule 2: Vertical legs connect boxes — check for | continuity between layers."""
    if len(boxes) < 2:
        return

    # Group boxes by layer (y coordinate)
    by_y = sorted(set(b.y for b in boxes))

    for i in range(len(by_y) - 1):
        upper_y = by_y[i]
        lower_y = by_y[i + 1]
        upper_boxes = [b for b in boxes if b.y == upper_y]
        lower_boxes = [b for b in boxes if b.y == lower_y]

        # Skip nested boxes (inner accepting border) — they're at the same
        # layer as the outer border, not a separate state.
        is_nested = lambda b: any(
            other != b and other.x <= b.x and other.right >= b.right
            and other.y <= b.y and other.bottom >= b.bottom
            for other in boxes
        )
        upper_boxes = [b for b in upper_boxes if not is_nested(b)]
        lower_boxes = [b for b in lower_boxes if not is_nested(b)]

        if not upper_boxes or not lower_boxes:
            continue

        # For each lower box, check if there's a vertical connection from above
        for lb in lower_boxes:
            cx = lb.cx
            # Check if there's a | or corridor connecting to this box
            found_connection = False
            for y in range(upper_boxes[0].bottom if upper_boxes else 0, lb.y):
                if y >= len(lines):
                    break
                line = lines[y]
                if cx < len(line) and line[cx] in ('|', '+', 'v', 'V', '▼', '-'):
                    found_connection = True
                    break
                # Also check nearby columns (corridor might shift cx slightly)
                for dx in range(-2, 3):
                    ncx = cx + dx
                    if 0 <= ncx < len(line) and line[ncx] in ('|', '+', '-'):
                        found_connection = True
                        break
                if found_connection:
                    break

            result.check(found_connection,
                f"No vertical connection found to '{lb.label}' (cx={cx}) "
                f"between y={upper_y} and y={lower_y}")


def verify_arrows_point_to_targets(boxes: List[Box], lines: List[str], result: ValidationResult):
    """Rule 6: Arrow v/▼ should be at or just above to box's center column."""
    arrow_chars = {'v', 'V', '▼', '^'}
    for b in boxes:
        cx = b.cx
        # Check row just above the box and the box's own top border
        found = False
        for check_y in [b.y - 1, b.y]:
            if 0 <= check_y < len(lines):
                line = lines[check_y]
                if cx < len(line) and line[cx] in arrow_chars:
                    found = True
                    break
                # Also check nearby columns
                for dx in range(-3, 4):
                    ncx = cx + dx
                    if 0 <= ncx < len(line) and line[ncx] in arrow_chars:
                        found = True
                        break
                if found:
                    break
        result.check(found or b.y <= 2,
            f"No arrow (v/▼) found at or above '{b.label}' (cx={cx}, y={b.y})")


def run_verification(output_text: str, labels: List[str] = None) -> ValidationResult:
    """Run all verification rules on figo output text."""
    result = ValidationResult()
    boxes, corridors, lines = parse_output(output_text)

    if not boxes:
        result.check(False, "No boxes found in output")
        return result

    verify_boxes_no_overlap(boxes, result)
    verify_box_spacing(boxes, result)
    verify_vertical_legs(boxes, lines, result)
    verify_arrows_point_to_targets(boxes, lines, result)

    if labels:
        verify_labels_not_wrapped(lines, labels, result)

    return result


def main():
    parser = argparse.ArgumentParser(description="Verify figo state diagram layout")
    parser.add_argument("output_file", nargs="?", help="Path to figo output text file")
    parser.add_argument("--figo", default=None, help="Path to figo.exe")
    parser.add_argument("--json", default=None, help="JSON input file for figo")
    parser.add_argument("--subcommand", default="state", help="figo subcommand (state/flowchart/etc)")
    parser.add_argument("--labels", nargs="*", default=[], help="Labels that should appear unsplit")
    args = parser.parse_args()

    # Get output text
    if args.output_file:
        with open(args.output_file, 'r', encoding='utf-8') as f:
            output_text = f.read()
    elif args.figo and args.json:
        with open(args.json, 'r', encoding='utf-8') as f:
            json_input = f.read()
        proc = subprocess.run([args.figo, args.subcommand], input=json_input,
                             capture_output=True, text=True)
        output_text = proc.stdout
    else:
        # Read from stdin
        output_text = sys.stdin.read()

    # Extract labels from JSON if provided
    labels = args.labels
    if args.json:
        with open(args.json, 'r', encoding='utf-8') as f:
            data = json.load(f)
        if 'transitions' in data:
            for t in data['transitions']:
                if 'label' in t:
                    labels.append(t['label'])

    result = run_verification(output_text, labels)
    print(result.summary())

    # Also print parsed boxes for debugging
    boxes, _, _ = parse_output(output_text)
    if boxes:
        print(f"\nParsed {len(boxes)} boxes:")
        for b in boxes:
            print(f"  '{b.label}' at ({b.x},{b.y}) size {b.w}x{b.h} cx={b.cx}")

    sys.exit(1 if result.failed > 0 else 0)


if __name__ == "__main__":
    main()
