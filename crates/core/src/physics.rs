//! Физика опор (фаза D): поиск поверхности, на которой стоит питомец.
//!
//! Модель — как в XPenguins: питомцы стоят ТОЛЬКО на верхних кромках окон
//! (и на земле). Stacking order не важен: из всех кромок под питомцем
//! побеждает самая высокая. Платформы поставляет worldsense-провайдер
//! демона, ядро о протоколах ничего не знает.

use crate::geometry::{Rect, Vec2};
use crate::pet::{Direction, World};

/// Платформа, по которой можно ходить: верхняя кромка окна.
/// `id` — стабильный идентификатор окна от провайдера (для отладки/логов;
/// сама физика опирается только на геометрию).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub rect: Rect,
    pub id: u64,
}

/// Поверхность, к которой прижат питомец (фаза G: стены и потолок).
///
/// Опорная точка `Pet::pos` — всегда точка касания поверхности (центр той
/// кромки спрайта, которой питомец её касается), поэтому смена поверхности
/// не «телепортирует» питомца: меняется только раскладка [`Pet::bounds`]
/// и ориентация кадра.
///
/// Направление движения вдоль поверхности задаётся [`Surface::tangent`] при
/// `facing = Right`; знак `facing` его переворачивает. Тангенсы выбраны так,
/// чтобы «нос» повёрнутого кадра всегда смотрел вперёд по движению:
/// на левой стене `Right` — это вниз, на правой — вверх (см. [`Surface::orient`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surface {
    /// Земля или верхняя кромка окна: ноги вниз (модель фаз M0–D).
    #[default]
    Floor,
    /// Потолок экрана: питомец висит под ним ногами вверх.
    Ceiling,
    /// Левая стена экрана: ноги влево, тело вправо от кромки.
    WallLeft,
    /// Правая стена экрана: ноги вправо, тело влево от кромки.
    WallRight,
}

/// Ориентация кадра при отрисовке: локальные отражения спрайта, затем
/// поворот на `quarter_turns` четвертей по часовой стрелке.
/// Кадры пака нарисованы мордой вправо и ногами вниз — всё остальное
/// получается этой трансформацией, отдельного арта под стены не нужно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orient {
    /// Отразить по горизонтали в системе координат кадра (смотрит влево).
    pub flip_x: bool,
    /// Отразить по вертикали в системе координат кадра (висит вниз головой).
    pub flip_y: bool,
    /// Поворот по часовой стрелке, 0..=3 четверти.
    pub quarter_turns: u8,
}

impl Orient {
    pub const IDENTITY: Orient = Orient {
        flip_x: false,
        flip_y: false,
        quarter_turns: 0,
    };

    /// Только зеркало по горизонтали (совместимость с прежним `mirror`).
    pub const fn mirrored(flip_x: bool) -> Orient {
        Orient {
            flip_x,
            flip_y: false,
            quarter_turns: 0,
        }
    }

    /// Меняет ли трансформация местами ширину и высоту кадра.
    pub const fn swaps_axes(self) -> bool {
        self.quarter_turns % 2 == 1
    }
}

impl Surface {
    /// Единичный вектор движения вдоль поверхности при `facing = Right`.
    pub fn tangent(self) -> Vec2 {
        match self {
            Surface::Floor | Surface::Ceiling => Vec2::new(1.0, 0.0),
            // Согласовано с orient(): после поворота «нос» кадра смотрит
            // вниз на левой стене и вверх на правой.
            Surface::WallLeft => Vec2::new(0.0, 1.0),
            Surface::WallRight => Vec2::new(0.0, -1.0),
        }
    }

    /// Нормаль: от поверхности в свободное пространство (куда смотрит спина).
    pub fn normal(self) -> Vec2 {
        match self {
            Surface::Floor => Vec2::new(0.0, -1.0),
            Surface::Ceiling => Vec2::new(0.0, 1.0),
            Surface::WallLeft => Vec2::new(1.0, 0.0),
            Surface::WallRight => Vec2::new(-1.0, 0.0),
        }
    }

    /// Движение идёт по горизонтали (пол и потолок) или по вертикали (стены).
    pub fn horizontal(self) -> bool {
        matches!(self, Surface::Floor | Surface::Ceiling)
    }

    /// Ориентация кадра: ноги всегда упираются в поверхность.
    pub fn orient(self, facing: Direction) -> Orient {
        let flip_x = facing == Direction::Left;
        match self {
            Surface::Floor => Orient {
                flip_x,
                flip_y: false,
                quarter_turns: 0,
            },
            // Потолок — вертикальное отражение: ноги вверх, направление взгляда
            // по горизонтали сохраняется.
            Surface::Ceiling => Orient {
                flip_x,
                flip_y: true,
                quarter_turns: 0,
            },
            // Поворот на 90° по часовой: низ кадра уходит влево — ноги в стену.
            Surface::WallLeft => Orient {
                flip_x,
                flip_y: false,
                quarter_turns: 1,
            },
            // Против часовой (три четверти по часовой): низ кадра уходит вправо.
            Surface::WallRight => Orient {
                flip_x,
                flip_y: false,
                quarter_turns: 3,
            },
        }
    }

    /// Прямоугольник спрайта размера `size` при касании в точке `pos`.
    pub fn bounds(self, pos: Vec2, size: f32) -> Rect {
        match self {
            Surface::Floor => Rect::new(pos.x - size / 2.0, pos.y - size, size, size),
            Surface::Ceiling => Rect::new(pos.x - size / 2.0, pos.y, size, size),
            Surface::WallLeft => Rect::new(pos.x, pos.y - size / 2.0, size, size),
            Surface::WallRight => Rect::new(pos.x - size, pos.y - size / 2.0, size, size),
        }
    }
}

/// Порог «ступеньки» при ходьбе, px: перепад опоры в пределах порога
/// перешагивается (снап вверх/вниз), больший обрыв вниз — падение.
pub const STEP_SNAP: f32 = 12.0;

/// Допуск «опора всё ещё под ногами», px: при изменении мира опора,
/// уехавшая по вертикали не дальше допуска, догоняется снапом,
/// дальше — питомец падает.
pub const SUPPORT_TOL: f32 = 4.0;

/// Y самой высокой опоры под точкой (x, y): верхняя кромка платформы,
/// лежащая не выше y (ось Y вниз, «ниже питомца» = top >= y), при
/// горизонтальном попадании x в [rect.x, rect.right()] включительно.
/// Если таких кромок нет — земля (`World::ground_y`, она же верх панели
/// при override). Земля возвращается и когда она выше y — вызывающий
/// сам решает, что делать с питомцем ниже пола.
pub fn support_below(world: &World, x: f32, y: f32) -> f32 {
    let mut best = world.ground_y();
    for p in &world.platforms {
        let top = p.rect.y;
        if x >= p.rect.x && x <= p.rect.right() && top >= y && top < best {
            best = top;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn world_with(platforms: Vec<Platform>) -> World {
        let mut w = World::new(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        w.platforms = platforms;
        w
    }

    fn plat(x: f32, y: f32, w: f32, id: u64) -> Platform {
        // Высота окна для опоры не важна — стоим только на верхней кромке.
        Platform {
            rect: Rect::new(x, y, w, 400.0),
            id,
        }
    }

    #[test]
    fn ground_when_no_platforms() {
        let w = world_with(vec![]);
        assert_eq!(support_below(&w, 500.0, 100.0), w.ground_y());
    }

    /// Перекрывающиеся окна: stacking не важен, побеждает самая высокая
    /// верхняя кромка ниже питомца.
    #[test]
    fn highest_top_edge_wins_for_overlapping_windows() {
        let w = world_with(vec![
            plat(100.0, 600.0, 800.0, 1),
            plat(200.0, 500.0, 800.0, 2),
        ]);
        assert_eq!(support_below(&w, 400.0, 100.0), 500.0);
        // Там, где верхнее окно не тянется, — кромка нижнего.
        assert_eq!(support_below(&w, 150.0, 100.0), 600.0);
    }

    /// Кромки выше питомца (top < y) — не опора: питомец падает мимо них
    /// на землю. Окна вне горизонтального диапазона тоже игнорируются.
    #[test]
    fn edges_above_or_aside_are_ignored() {
        let w = world_with(vec![plat(100.0, 300.0, 200.0, 1)]);
        // Питомец ниже кромки — окно сверху не считается.
        assert_eq!(support_below(&w, 150.0, 400.0), w.ground_y());
        // Питомец в стороне от окна.
        assert_eq!(support_below(&w, 500.0, 100.0), w.ground_y());
    }

    /// Границы кромки включительны: x == rect.x и x == rect.right() — опора.
    #[test]
    fn edge_x_bounds_are_inclusive() {
        let w = world_with(vec![plat(100.0, 500.0, 200.0, 1)]);
        assert_eq!(support_below(&w, 100.0, 0.0), 500.0);
        assert_eq!(support_below(&w, 300.0, 0.0), 500.0);
        assert_eq!(support_below(&w, 300.1, 0.0), w.ground_y());
    }

    /// Верх панели (exclusive-зона, D5) — пол: без платформ опора = override.
    #[test]
    fn panel_override_is_the_ground() {
        let mut w = world_with(vec![]);
        w.ground_y_override = Some(1040.0);
        assert_eq!(support_below(&w, 500.0, 100.0), 1040.0);
        // Платформа выше панели всё равно побеждает.
        w.platforms = vec![plat(0.0, 700.0, 1920.0, 1)];
        assert_eq!(support_below(&w, 500.0, 100.0), 700.0);
    }
}
