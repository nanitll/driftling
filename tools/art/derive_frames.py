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
    # Держится за поверхность: лапки укорочены в хват.
    out["cling_0"] = drop_legs(idle, keep=1)
    out["cling_1"] = shift_rows(drop_legs(idle, keep=1), antenna, +1)
    # Ползёт: тот же шаг, но лапки в хвате.
    for i in range(4):
        out[f"climb_{i}"] = drop_legs(src[f"walk_{i}"], keep=1)
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
"""


def sync_manifest() -> None:
    """Дописать в pack.toml секции новых семейств (идемпотентно)."""
    path = PACK / "pack.toml"
    text = path.read_text()
    for stage in STAGES:
        if f"[stages.{stage}.anims.blink]" in text:
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
