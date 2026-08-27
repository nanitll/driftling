//! Физика опор (фаза D): поиск поверхности, на которой стоит питомец.
//!
//! Модель — как в XPenguins: питомцы стоят ТОЛЬКО на верхних кромках окон
//! (и на земле). Stacking order не важен: из всех кромок под питомцем
//! побеждает самая высокая. Платформы поставляет worldsense-провайдер
//! демона, ядро о протоколах ничего не знает.

use crate::geometry::Rect;
use crate::pet::World;

/// Платформа, по которой можно ходить: верхняя кромка окна.
/// `id` — стабильный идентификатор окна от провайдера (для отладки/логов;
/// сама физика опирается только на геометрию).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub rect: Rect,
    pub id: u64,
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
