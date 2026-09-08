#!/usr/bin/env python3
"""Генератор производных кадров пака (фаза G).

Новые семейства анимаций получаются трансформациями уже нарисованных
кадров: моргание, посадка, потягивание, шевеление антенной, хват за
стену и звёздочки после удара о потолок. Так новый арт остаётся в стиле
базовых кадров на всех стадиях роста и не расходится с ними при правке.

Запуск (из корня репозитория):

    python3 tools/art/derive_frames.py            # переписать кадры
    python3 tools/art/derive_frames.py --check    # только проверить

Разметка кадра читается из самой сетки (см. `Anatomy`): антенна — всё,
что выше корпуса, лапки — ряды ниже нижнего контура корпуса, глаза —
связные области с бликом «S» (рот бликов не имеет и потому не трогается).
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACK = ROOT / "assets" / "pack-default"
STAGES = ["baby", "child", "teen", "adult"]
EMPTY = "."
BODY = "B"
EYE = "E"
SHINE = "S"
STAR = "S"

Grid = list[list[str]]


def read_grid(path: Path) -> Grid:
    rows = [list(line) for line in path.read_text().splitlines() if line.strip()]
    width = len(rows[0])
    assert all(len(r) == width for r in rows), f"{path}: рваная сетка"
    return rows


def write_grid(path: Path, grid: Grid) -> None:
    path.write_text("\n".join("".join(r) for r in grid) + "\n")


def blank_row(width: int) -> list[str]:
    return [EMPTY] * width


def row_filled(row: list[str]) -> int:
    return sum(1 for c in row if c != EMPTY)


@dataclass
class Anatomy:
    """Разметка кадра: где антенна, корпус и лапки."""

    body_top: int  # первый ряд корпуса
    body_bottom: int  # последний ряд корпуса (нижний контур)
    legs: list[int]  # ряды лапок под корпусом


def row_is_solid(row: list[str]) -> bool:
    """Ряд корпуса — сплошная полоса; у ряда лапок посередине просвет."""
    xs = [x for x, c in enumerate(row) if c != EMPTY]
    return bool(xs) and len(xs) == xs[-1] - xs[0] + 1


def anatomy(grid: Grid) -> Anatomy:
    """Корпус — самая широкая часть кадра; лапки под ним разрежены.

    Ряд с максимальной заливкой — середина корпуса. Вниз идём до первого
    ряда с просветом посередине: это уже лапки. Вверх — до ряда вдвое уже
    максимума: это шея и антенна.
    """
    fills = [row_filled(r) for r in grid]
    widest = max(range(len(grid)), key=lambda i: fills[i])
    peak = fills[widest]

    body_bottom = widest
    for i in range(widest, len(grid)):
        if fills[i] == 0 or not row_is_solid(grid[i]):
            break
        body_bottom = i
    body_top = widest
    for i in range(widest, -1, -1):
        if fills[i] == 0 or fills[i] < peak * 0.45:
            break
        body_top = i

    legs = [i for i in range(body_bottom + 1, len(grid)) if fills[i] > 0]
    return Anatomy(body_top=body_top, body_bottom=body_bottom, legs=legs)


def eye_clusters(grid: Grid) -> list[list[tuple[int, int]]]:
    """Связные области глаз: те, в которых есть блик «S» (у рта его нет)."""
    height, width = len(grid), len(grid[0])
    seen: set[tuple[int, int]] = set()
    clusters: list[list[tuple[int, int]]] = []
    for y in range(height):
        for x in range(width):
            if grid[y][x] not in (EYE, SHINE) or (y, x) in seen:
                continue
            stack, cells = [(y, x)], []
            seen.add((y, x))
            while stack:
                cy, cx = stack.pop()
                cells.append((cy, cx))
                for ny, nx in ((cy - 1, cx), (cy + 1, cx), (cy, cx - 1), (cy, cx + 1)):
                    if (
                        0 <= ny < height
                        and 0 <= nx < width
                        and (ny, nx) not in seen
                        and grid[ny][nx] in (EYE, SHINE)
                    ):
                        seen.add((ny, nx))
                        stack.append((ny, nx))
            if any(grid[cy][cx] == SHINE for cy, cx in cells):
                clusters.append(cells)
    return clusters


def close_eyes(grid: Grid) -> Grid:
    """Закрытые глаза: вместо зрачка — дужка по центру глазницы."""
    out = [row[:] for row in grid]
    for cells in eye_clusters(out):
        ys = [y for y, _ in cells]
        xs = [x for _, x in cells]
        fill = out[max(ys)][min(xs) - 1] if min(xs) > 0 else BODY
        if fill in (EYE, SHINE):
            fill = BODY
        for y, x in cells:
            out[y][x] = fill
        mid = (min(ys) + max(ys)) // 2
        for x in range(min(xs), max(xs) + 1):
            out[mid][x] = EYE
    return out


def shift_rows(grid: Grid, rows: range | list[int], dx: int) -> Grid:
    """Сдвинуть указанные ряды по горизонтали (антенна «водит»)."""
    out = [row[:] for row in grid]
    width = len(grid[0])
    for y in rows:
        src = grid[y]
        new = blank_row(width)
        for x, ch in enumerate(src):
            if ch == EMPTY:
                continue
            nx = x + dx
            if 0 <= nx < width:
                new[nx] = ch
        out[y] = new
    return out


def drop_legs(grid: Grid, keep: int) -> Grid:
    """Убрать лапки, оставив `keep` рядов, и опустить корпус на их место.

    Нижний ряд спрайта — «подошва» питомца: она обязана остаться на месте,
    иначе на экране он повиснет над полом.
    """
    an = anatomy(grid)
    removed = max(0, len(an.legs) - keep)
    if removed == 0:
        return [row[:] for row in grid]
    height, width = len(grid), len(grid[0])
    out = [blank_row(width) for _ in range(height)]
    for y in range(0, an.body_bottom + 1):
        ny = y + removed
        if 0 <= ny < height:
            out[ny] = grid[y][:]
    for i, y in enumerate(an.legs[:keep]):
        ny = y + removed
        if 0 <= ny < height:
            out[ny] = grid[y][:]
    return out


def stretch_up(grid: Grid) -> Grid:
    """Потягивание: корпус на цыпочках — тело вверх, лапки длиннее."""
    an = anatomy(grid)
    if not an.legs:
        return [row[:] for row in grid]
    height, width = len(grid), len(grid[0])
    out = [blank_row(width) for _ in range(height)]
    # Корпус и антенна поднимаются на ряд.
    for y in range(0, an.body_bottom + 1):
        ny = y - 1
        if 0 <= ny < height:
            out[ny] = grid[y][:]
    # Лапки растягиваются: верхний ряд лапок повторяется лишний раз,
    # носок (последний ряд) остаётся единственным.
    legs = [grid[an.legs[0]][:]] + [grid[y][:] for y in an.legs]
    for i, row in enumerate(legs):
        y = an.body_bottom + i
        if y < height:
            out[y] = row
    return out


def cells_of(grid: Grid, ch: str) -> list[tuple[int, int]]:
    return [(y, x) for y, row in enumerate(grid) for x, c in enumerate(row) if c == ch]


def move_cells(grid: Grid, cells: list[tuple[int, int]], dx: int, dy: int = 0) -> None:
    """Перенести пиксели внутри силуэта: исходные затираются телом, новые
    рисуются только там, где под ними уже есть тело (за контур не вылезаем)."""
    taken = [(y, x, grid[y][x]) for y, x in cells]
    for y, x, _ in taken:
        grid[y][x] = BODY
    for y, x, ch in taken:
        ny, nx = y + dy, x + dx
        if 0 <= ny < len(grid) and 0 <= nx < len(grid[0]) and grid[ny][nx] in (BODY, "D"):
            grid[ny][nx] = ch


def profile_view(grid: Grid) -> Grid:
    """Профиль (вид сбоку), мордой вправо.

    Питомец нарисован анфас, но на стене он должен быть виден именно сбоку —
    поэтому дальний глаз и дальняя щека убираются, ближние съезжают к морде,
    брюшко уходит вперёд, а дальняя лапка прячется за ближней и темнеет.
    """
    out = [row[:] for row in grid]
    width = len(out[0])
    center = width // 2

    # Глаза: дальний убираем, ближний остаётся у морды.
    clusters = eye_clusters(out)
    eye_cells = {c for cluster in clusters for c in cluster}
    if clusters:
        near = max(clusters, key=lambda c: sum(x for _, x in c) / len(c))
        for cells in clusters:
            if cells is near:
                continue
            for y, x in cells:
                out[y][x] = BODY
        # Ближний глаз и так стоит у морды — двигать его некуда,
        # дальше только контур.

    # Щёки: остаётся только ближняя.
    for y, x in cells_of(out, "C"):
        if x < center:
            out[y][x] = BODY
    # Рот уезжает к морде. Контур глаза тоже нарисован символом «E» —
    # его трогать нельзя, иначе морда рассыпается.
    mouth = [(y, x) for y, x in cells_of(out, EYE) if (y, x) not in eye_cells]
    move_cells(out, mouth, +4)
    # Брюшко видно спереди — сдвигаем вперёд.
    move_cells(out, cells_of(out, "W"), +3)
    # Антенна чуть заваливается назад.
    an = anatomy(out)
    out = shift_rows(out, list(range(0, an.body_top)), -1)

    # Дальняя лапка прячется за ближнюю и уходит в тень.
    an = anatomy(out)
    groups = foot_columns(out, an.legs)
    if len(groups) >= 2:
        far_x0, far_x1 = groups[0]
        far = [
            (y, x)
            for y in an.legs
            for x in range(far_x0, far_x1 + 1)
            if out[y][x] != EMPTY
        ]
        shade = [(y, x, "D" if out[y][x] == BODY else out[y][x]) for y, x in far]
        for y, x in far:
            out[y][x] = EMPTY
        for y, x, ch in shade:
            nx = x + 3
            if 0 <= nx < width and out[y][nx] == EMPTY:
                out[y][nx] = ch
    return out


def climb_profile(grid: Grid, near_up: int, far_up: int) -> Grid:
    """Поза лазания в профиль: питомец висит вертикально, мордой к стене,
    лапки цепляются за неё спереди. Стена — слева от кадра после зеркала,
    поэтому лапки рисуем со стороны морды.

    `near_up`/`far_up` — на сколько рядов подтянута передняя и задняя лапка.
    """
    # Лапки, на которых он ходит по полу, поджаты: на стене все четыре
    # держатся за неё, а не болтаются вниз.
    out = drop_legs(profile_view(grid), keep=0)
    an = anatomy(out)
    height = max(4, an.body_bottom - an.body_top)
    # Лапки цепляются спереди: верхняя — у плеча, нижняя — у бедра.
    draw_paw(out, an.body_top + height // 4 - near_up, on_left=False)
    draw_paw(out, an.body_bottom - height // 3 - far_up, on_left=False)
    return out


def back_view(grid: Grid) -> Grid:
    """Вид со спины: питомец прижался к поверхности, лица и брюшка не видно.

    Именно так он выглядит, когда лезет по стене или висит под потолком, —
    поворачивать кадр боком (как на первой итерации фазы G) неправдоподобно.
    """
    out = [row[:] for row in grid]
    for y, row in enumerate(out):
        for x, ch in enumerate(row):
            # Глаза, блики, щёки, рот и светлое брюшко — всё это спереди.
            if ch in (EYE, SHINE, "C", "W"):
                out[y][x] = BODY
    return out


def side_margin(grid: Grid, y: int) -> tuple[int, int]:
    """Крайние занятые пиксели ряда (слева, справа); (-1, -1) — ряд пуст."""
    xs = [x for x, c in enumerate(grid[y]) if c != EMPTY]
    return (xs[0], xs[-1]) if xs else (-1, -1)


def paw_row(grid: Grid, lo: int, hi: int) -> int:
    """Ряд в полосе [lo, hi], где силуэт уже всего: там лапка не сольётся с телом."""
    best, best_w = lo, 10**6
    for y in range(max(0, lo), min(hi + 1, len(grid))):
        left, right = side_margin(grid, y)
        if left < 0:
            continue
        if right - left < best_w:
            best, best_w = y, right - left
    return best


def outline_around(grid: Grid, cells: list[tuple[int, int]]) -> None:
    """Обвести контуром только что нарисованные пиксели тела."""
    height, width = len(grid), len(grid[0])
    for y, x in cells:
        for ny, nx in ((y - 1, x), (y + 1, x), (y, x - 1), (y, x + 1)):
            if 0 <= ny < height and 0 <= nx < width and grid[ny][nx] == EMPTY:
                grid[ny][nx] = "O"


def draw_paw(grid: Grid, y: int, on_left: bool) -> None:
    """Лапка сбоку от корпуса: 2x2 «варежка» вплотную к силуэту, со своим
    контуром. Ряд за рядом лапка повторяет изгиб тела и не отлипает."""
    width = len(grid[0])
    painted: list[tuple[int, int]] = []
    for dy in (0, 1):
        row = y + dy
        if not 0 <= row < len(grid):
            continue
        left, right = side_margin(grid, row)
        if left < 0:
            continue
        for dx in (1, 2):
            x = (left - dx) if on_left else (right + dx)
            if 0 <= x < width and grid[row][x] == EMPTY:
                grid[row][x] = BODY
                painted.append((row, x))
    outline_around(grid, painted)


def foot_columns(grid: Grid, legs: list[int]) -> list[tuple[int, int]]:
    """Колонки левой и правой лапки (по просвету между ними)."""
    cols = sorted({x for y in legs for x, c in enumerate(grid[y]) if c != EMPTY})
    if not cols:
        return []
    groups, start, prev = [], cols[0], cols[0]
    for x in cols[1:]:
        if x - prev > 1:
            groups.append((start, prev))
            start = x
        prev = x
    groups.append((start, prev))
    return groups


def shift_feet(grid: Grid, offsets: list[int]) -> Grid:
    """Поднять/опустить лапки: шаг лазания. `offsets` — по лапке слева направо."""
    an = anatomy(grid)
    if not an.legs:
        return [row[:] for row in grid]
    groups = foot_columns(grid, an.legs)
    out = [row[:] for row in grid]
    for i, (x0, x1) in enumerate(groups):
        dy = offsets[i] if i < len(offsets) else 0
        if dy == 0:
            continue
        cells = [
            (y, x, grid[y][x])
            for y in an.legs
            for x in range(x0, x1 + 1)
            if grid[y][x] != EMPTY
        ]
        for y, x, _ in cells:
            out[y][x] = EMPTY
        for y, x, ch in cells:
            ny = y + dy
            if 0 <= ny < len(out):
                out[ny][x] = ch
    return out


def climb_pose(grid: Grid, paw_up: int, feet: list[int]) -> Grid:
    """Кадр лазания по СТЕНЕ. Кадр рисуется как обычно (мордой к зрителю,
    ногами вниз), а на стене рендер поворачивает его на четверть — питомец
    оказывается боком, упираясь лапками в стену. Поэтому лицо здесь на
    месте: со спины, без морды, это выглядело как чужое существо.

    Лапки на кадре — «передние» (те, что после поворота тянутся вверх по
    стене): одна цепляется выше другой, чтобы шаг читался.
    """
    out = [row[:] for row in grid]
    an = anatomy(out)
    height = max(2, an.body_bottom - an.body_top)
    # Лапки крепим в верхней трети корпуса, где силуэт уже.
    upper = paw_row(out, an.body_top + 1, an.body_top + height // 3)
    draw_paw(out, upper + (0 if paw_up < 0 else 2), on_left=True)
    draw_paw(out, upper + (0 if paw_up > 0 else 2), on_left=False)
    return shift_feet(out, feet)


def arm_up(grid: Grid, on_left: bool, hand_row: int) -> None:
    """Рука, поднятая вверх: от плеча до `hand_row` (0 — держится за потолок),
    с кулачком наверху. Рисуется поверх пустоты, тело не трогает."""
    an = anatomy(grid)
    height = len(grid)
    width = len(grid[0])
    shoulder = an.body_top + max(2, (an.body_bottom - an.body_top) // 5)
    left, right = side_margin(grid, shoulder)
    if left < 0:
        return
    x = (left + 2) if on_left else (right - 3)
    x = max(1, min(width - 3, x))
    for r in range(max(0, hand_row + 2), shoulder + 1):
        for dx in (0, 1):
            if grid[r][x + dx] == EMPTY:
                grid[r][x + dx] = BODY
        for dx in (-1, 2):
            if 0 <= x + dx < width and grid[r][x + dx] == EMPTY:
                grid[r][x + dx] = "O"
    # Кулачок: 4x2 сверху.
    for r in range(max(0, hand_row), min(height, hand_row + 2)):
        for dx in (-1, 0, 1, 2):
            if 0 <= x + dx < width and grid[r][x + dx] == EMPTY:
                grid[r][x + dx] = "O" if r == hand_row or dx in (-1, 2) else BODY
    if hand_row + 2 < height:
        for dx in (0, 1):
            if grid[hand_row + 2][x + dx] == EMPTY:
                grid[hand_row + 2][x + dx] = BODY


def hang_pose(grid: Grid, left_hand: int, right_hand: int, sway: int) -> Grid:
    """Висит под потолком на лапках, как обезьянка: тело качается (`sway`
    px вбок), руки тянутся к потолку; `*_hand` — ряд кулачка (0 — держит)."""
    an = anatomy(grid)
    body = shift_rows(grid, list(range(0, len(grid))), sway)
    # Ноги болтаются: чуть в противофазе к телу.
    body = shift_rows(body, an.legs, -sway)
    arm_up(body, on_left=True, hand_row=left_hand)
    arm_up(body, on_left=False, hand_row=right_hand)
    return body


def wave_pose(grid: Grid, hand_row_offset: int) -> Grid:
    """Приветственный взмах: ближняя лапка поднята к макушке и машет."""
    out = [row[:] for row in grid]
    an = anatomy(out)
    top = next(y for y, row in enumerate(out) if row_filled(row) > 0)
    arm_up(out, on_left=False, hand_row=max(0, top + hand_row_offset))
    # Слегка приподнятая на цыпочки поза читается как «тянется помахать».
    if an.legs:
        out = shift_feet(out, [0, -1])
    return out


def dangle_pose(grid: Grid, swing: int) -> Grid:
    """Сидит на карнизе, свесив лапки: корпус опущен, лапки — вперёд-вниз
    и качаются (`swing` — сдвиг лапок по горизонтали)."""
    an = anatomy(grid)
    if not an.legs:
        return [row[:] for row in grid]
    height, width = len(grid), len(grid[0])
    out = [blank_row(width) for _ in range(height)]
    # Корпус садится на ряд ниже.
    for y in range(0, an.body_bottom + 1):
        ny = y + 1
        if ny < height:
            out[ny] = grid[y][:]
    # Лапки — вперёд (вправо, к краю карниза) и качаются.
    groups = foot_columns(grid, an.legs)
    shift = 3 + swing
    for y in an.legs:
        for x in range(width):
            ch = grid[y][x]
            if ch == EMPTY:
                continue
            near = groups and x >= groups[-1][0]
            dx = shift if near else shift - 2
            nx = x + dx
            if 0 <= nx < width and out[y][nx] == EMPTY:
                out[y][nx] = ch if near else ("D" if ch == BODY else ch)
    return out


def vomit_pose(grid: Grid, splash: bool) -> Grid:
    """Тошнит: глаза зажмурены, рот распахнут; на втором кадре — струйка."""
    out = close_eyes(grid)
    clusters = eye_clusters(grid)
    eye_cells = {c for cluster in clusters for c in cluster}
    mouth = [(y, x) for y, x in cells_of(grid, EYE) if (y, x) not in eye_cells]
    if not mouth:
        return out
    cy = sum(y for y, _ in mouth) // len(mouth)
    cx = sum(x for _, x in mouth) // len(mouth)
    for y in range(cy - 1, cy + 3):
        for x in range(cx - 2, cx + 3):
            if 0 <= y < len(out) and 0 <= x < len(out[0]) and out[y][x] in (BODY, EYE, "D", "W"):
                out[y][x] = "X"
    if splash:
        for y, x in ((cy + 3, cx), (cy + 4, cx - 1), (cy + 4, cx + 1), (cy + 5, cx)):
            if 0 <= y < len(out) and 0 <= x < len(out[0]) and out[y][x] in (BODY, "W", "D", EMPTY):
                out[y][x] = "A"
    return out


def add_stars(grid: Grid, centers: list[tuple[int, int]]) -> Grid:
    """Звёздочки-крестики над головой (кадры оглушения).

    Отсчёт — от макушки кадра: `centers` задаёт (сколько рядов выше
    макушки, смещение от центра по горизонтали).
    """
    out = [row[:] for row in grid]
    width = len(out[0])
    top = next(y for y, row in enumerate(out) if row_filled(row) > 0)
    for up, dx in centers:
        cy, cx = top - up, width // 2 + dx
        for y, x in ((cy, cx), (cy - 1, cx), (cy + 1, cx), (cy, cx - 1), (cy, cx + 1)):
            if 0 <= y < len(out) and 0 <= x < width and out[y][x] == EMPTY:
                out[y][x] = STAR
    return out


def derive(stage: str) -> dict[str, Grid]:
    """Все производные кадры одной стадии."""
    src = {
        name: read_grid(PACK / stage / f"{name}.txt")
        for name in ("idle_0", "idle_1", "walk_0", "walk_1", "walk_2", "walk_3", "landing_0")
    }
    idle = src["idle_0"]
    an = anatomy(idle)
    antenna = list(range(0, an.body_top))

    out: dict[str, Grid] = {}
    # Моргание — один кадр: глаза закрыты.
    out["blink_0"] = close_eyes(idle)
    # Сидит: лапки поджаты, корпус опустился.
    out["sit_0"] = drop_legs(idle, keep=0)
    out["sit_1"] = drop_legs(src["idle_1"], keep=0)
    # Потягивается с зевком, затем расслабляется.
    out["stretch_0"] = close_eyes(stretch_up(idle))
    out["stretch_1"] = close_eyes(idle)
    # Топчется и водит антенной.
    out["wiggle_0"] = shift_rows(idle, antenna, +1)
    out["wiggle_1"] = shift_rows(src["idle_1"], antenna, -1)
    # Висит на стене без движения: обе лапки держат вровень.
    out["cling_0"] = climb_profile(idle, near_up=0, far_up=0)
    out["cling_1"] = climb_profile(src["idle_1"], near_up=0, far_up=0)
    # Лезет: лапки перехватывают по очереди — передняя тянется, задняя
    # подтягивается следом.
    out["climb_0"] = climb_profile(idle, near_up=2, far_up=0)
    out["climb_1"] = climb_profile(src["idle_1"], near_up=1, far_up=1)
    out["climb_2"] = climb_profile(idle, near_up=0, far_up=2)
    out["climb_3"] = climb_profile(src["idle_1"], near_up=1, far_up=1)
    # Под потолком — обезьянка: висит на лапках и качается; при движении
    # перехватывается рука за рукой.
    out["hang_0"] = hang_pose(idle, left_hand=0, right_hand=0, sway=0)
    out["hang_1"] = hang_pose(src["idle_1"], left_hand=0, right_hand=0, sway=1)
    out["swing_0"] = hang_pose(idle, left_hand=0, right_hand=3, sway=-1)
    out["swing_1"] = hang_pose(src["idle_1"], left_hand=0, right_hand=0, sway=0)
    out["swing_2"] = hang_pose(idle, left_hand=3, right_hand=0, sway=1)
    out["swing_3"] = hang_pose(src["idle_1"], left_hand=0, right_hand=0, sway=0)
    # Машет лапкой: здоровается и прощается.
    out["wave_0"] = wave_pose(idle, hand_row_offset=0)
    out["wave_1"] = wave_pose(src["idle_1"], hand_row_offset=2)
    # Сидит на карнизе, свесив лапки.
    out["dangle_0"] = dangle_pose(idle, swing=0)
    out["dangle_1"] = dangle_pose(src["idle_1"], swing=1)
    # Укачало: тошнит (зелёный оттенок добавляет демон перекраской).
    out["vomit_0"] = vomit_pose(idle, splash=False)
    out["vomit_1"] = vomit_pose(idle, splash=True)
    # Звёздочки после удара о потолок.
    dizzy = close_eyes(src["landing_0"])
    out["dizzy_0"] = add_stars(dizzy, [(2, -7), (5, 4)])
    out["dizzy_1"] = add_stars(dizzy, [(5, -5), (2, 6)])
    return out


MANIFEST_BLOCK = """
[stages.{stage}.anims.blink]
fps = 1.0
frames = ["blink_0"]

[stages.{stage}.anims.sit]
fps = 1.5
frames = ["sit_0", "sit_1"]

[stages.{stage}.anims.stretch]
fps = 2.0
frames = ["stretch_0", "stretch_1"]

[stages.{stage}.anims.wiggle]
fps = 4.0
frames = ["wiggle_0", "wiggle_1"]

[stages.{stage}.anims.cling]
fps = 1.5
frames = ["cling_0", "cling_1"]

[stages.{stage}.anims.climb]
fps = 5.0
frames = ["climb_0", "climb_1", "climb_2", "climb_3"]

[stages.{stage}.anims.dizzy]
fps = 4.0
frames = ["dizzy_0", "dizzy_1"]

[stages.{stage}.anims.hang]
fps = 1.5
frames = ["hang_0", "hang_1"]

[stages.{stage}.anims.swing]
fps = 5.0
frames = ["swing_0", "swing_1", "swing_2", "swing_3"]

[stages.{stage}.anims.vomit]
fps = 3.0
frames = ["vomit_0", "vomit_1"]

[stages.{stage}.anims.wave]
fps = 3.0
frames = ["wave_0", "wave_1"]

[stages.{stage}.anims.dangle]
fps = 1.2
frames = ["dangle_0", "dangle_1"]
"""


def sync_manifest() -> None:
    """Дописать в pack.toml секции новых семейств (идемпотентно)."""
    path = PACK / "pack.toml"
    text = path.read_text()
    for stage in STAGES:
        if f"[stages.{stage}.anims.wave]" in text:
            continue
        if f"[stages.{stage}.anims.hang]" in text:
            extra = MANIFEST_BLOCK.format(stage=stage)
            extra = extra[extra.index(f"[stages.{stage}.anims.wave]"):]
            text = text.rstrip("\n") + "\n\n" + extra
            continue
        if f"[stages.{stage}.anims.blink]" in text:
            # Блок фазы G уже есть — дописываем только новые семейства.
            extra = MANIFEST_BLOCK.format(stage=stage)
            extra = extra[extra.index(f"[stages.{stage}.anims.hang]"):]
            text = text.rstrip("\n") + "\n\n" + extra
            continue
        text = text.rstrip("\n") + "\n" + MANIFEST_BLOCK.format(stage=stage)
    path.write_text(text.rstrip("\n") + "\n")


def sync_frame_sources() -> None:
    """Пересобрать список include_str! встроенного пака в pack.rs.

    Кадры подключаются литералами (include_str! не умеет иначе), поэтому
    список ведётся генератором, а не руками.
    """
    path = ROOT / "crates" / "core" / "src" / "pack.rs"
    text = path.read_text()
    head, rest = text.split("static FRAME_SOURCES: &[(&str, &str)] = &[\n", 1)
    _, tail = rest.split("\n];\n", 1)

    entries = []
    for stage in ["egg", *STAGES]:
        for frame in sorted(p.stem for p in (PACK / stage).glob("*.txt")):
            key = f"{stage}/{frame}"
            entries.append(
                f'    (\n        "{key}",\n'
                f'        include_str!("../../../assets/pack-default/{key}.txt"),\n    ),'
            )
    body = "\n".join(entries)
    path.write_text(
        f"{head}static FRAME_SOURCES: &[(&str, &str)] = &[\n{body}\n];\n{tail}"
    )
    print(f"pack.rs: {len(entries)} кадров подключено")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="не писать, только сверить")
    args = ap.parse_args()

    stale = []
    for stage in STAGES:
        frames = derive(stage)
        native = len(read_grid(PACK / stage / "idle_0.txt"))
        for name, grid in frames.items():
            assert len(grid) == native and all(len(r) == native for r in grid), (
                f"{stage}/{name}: {len(grid)}x{len(grid[0])} вместо {native}"
            )
            path = PACK / stage / f"{name}.txt"
            text = "\n".join("".join(r) for r in grid) + "\n"
            if args.check:
                if not path.exists() or path.read_text() != text:
                    stale.append(f"{stage}/{name}")
            else:
                path.write_text(text)
        if not args.check:
            print(f"{stage}: {len(frames)} кадров")

    if args.check:
        if stale:
            print("кадры разошлись с генератором: " + ", ".join(stale))
            return 1
        print("все производные кадры актуальны")
        return 0

    sync_manifest()
    sync_frame_sources()
    return 0


if __name__ == "__main__":
    sys.exit(main())
