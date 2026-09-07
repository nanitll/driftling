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
/// `facing = Right`; знак `facing` его переворачивает. На стенах движение
/// вертикальное: на левой `Right` — вниз, на правой — вверх.
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
///
/// Питомец пользуется только отражениями: на стене он не лежит боком, а
/// держится за неё спиной к нам (для этого в паке есть свои кадры `climb`
/// и `cling`), под потолком висит вверх ногами. Повороты оставлены в
/// контракте рендера для паков и будущих поз — блит их умеет.
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

    /// Размер кадра `(w, h)` после трансформации.
    pub const fn output_size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Обратная трансформация: какой пиксель исходного кадра `w`x`h` лежит
    /// в точке `(dx, dy)` уже повёрнутого вывода. Сперва откручиваем
    /// поворот по часовой, затем снимаем зеркала.
    ///
    /// Это единственная реализация поворота в проекте: ею пользуются и
    /// CPU-рендер оверлея, и дев-инструменты — расхождению взяться неоткуда.
    pub fn source_pixel(self, w: u32, h: u32, dx: u32, dy: u32) -> (u32, u32) {
        let (fx, fy) = match self.quarter_turns % 4 {
            0 => (dx, dy),
            1 => (dy, h.saturating_sub(1) - dx.min(h.saturating_sub(1))),
            2 => (
                w.saturating_sub(1) - dx.min(w.saturating_sub(1)),
                h.saturating_sub(1) - dy.min(h.saturating_sub(1)),
            ),
            _ => (w.saturating_sub(1) - dy.min(w.saturating_sub(1)), dx),
        };
        let sx = if self.flip_x {
            w.saturating_sub(1) - fx.min(w.saturating_sub(1))
        } else {
            fx
        };
        let sy = if self.flip_y {
            h.saturating_sub(1) - fy.min(h.saturating_sub(1))
        } else {
            fy
        };
        (sx.min(w.saturating_sub(1)), sy.min(h.saturating_sub(1)))
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

    /// Ориентация кадра под поверхность.
    ///
    /// На полу — привычное зеркало по направлению взгляда. На стене питомец
    /// стоит вертикально и держится за неё спиной к зрителю (кадры `climb`/
    /// `cling`): поворачивать его боком неправдоподобно, а зеркалить нечего —
    /// поза симметрична, и мельтешение при смене направления только мешало бы.
    /// Под потолком тот же кадр отражается по вертикали — питомец висит,
    /// зацепившись лапками, антенной вниз.
    pub fn orient(self, facing: Direction) -> Orient {
        let flip_x = facing == Direction::Left;
        match self {
            Surface::Floor => Orient::mirrored(flip_x),
            Surface::Ceiling => Orient {
                flip_x,
                flip_y: true,
                quarter_turns: 0,
            },
            Surface::WallLeft | Surface::WallRight => Orient::IDENTITY,
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

/// Минимальная ширина карниза, на который питомца вообще пускают, px.
/// Кусочки уже этого — щели между окнами, а не место для жизни.
pub const MIN_LEDGE: f32 = 24.0;

/// Видимые участки верхних кромок окон (фаза G2).
///
/// На вход — окна В ПОРЯДКЕ СТЕКИНГА СВЕРХУ ВНИЗ (первое самое верхнее),
/// как их отдаёт worldsense. Кромка окна, накрытая другим окном, местом для
/// стояния не является: иначе питомец «стоит в воздухе» посреди чужого окна —
/// именно это и выглядит как сломанный поиск опоры.
///
/// Каждая кромка режется на видимые отрезки, и каждый отрезок становится
/// отдельной платформой (id сохраняется — платформы одного окна связаны).
/// Отрезки уже `min_width` выбрасываются, кромки за пределами экрана —
/// обрезаются по нему.
///
/// `head_room` — высота питомца: над кромкой должно остаться место, где его
/// видно. Кромка максимизированного окна лежит на самом верху экрана, и
/// питомец, «стоящий» на ней, оказался бы телом за верхним краем — то есть
/// невидимым. Такие кромки не платформы (наверху есть потолок — на нём
/// питомец висит и его видно).
pub fn visible_ledges(
    stack_top_first: &[Platform],
    screen: Rect,
    min_width: f32,
    head_room: f32,
) -> Vec<Platform> {
    let mut out = Vec::new();
    for (i, p) in stack_top_first.iter().enumerate() {
        let top = p.rect.y;
        // Кромка выше/ниже экрана или без места для питомца над ней.
        if top - head_room < screen.y || top > screen.bottom() {
            continue;
        }
        let mut spans = vec![(p.rect.x.max(screen.x), p.rect.right().min(screen.right()))];
        for above in &stack_top_first[..i] {
            let r = above.rect;
            // Окно накрывает кромку, только если пересекает её по вертикали:
            // верх окна не ниже кромки, низ — строго ниже неё.
            if r.y <= top && r.bottom() > top {
                spans = subtract_span(&spans, r.x, r.right());
                if spans.is_empty() {
                    break;
                }
            }
        }
        for (x0, x1) in spans {
            if x1 - x0 >= min_width {
                out.push(Platform {
                    rect: Rect::new(x0, top, x1 - x0, p.rect.h),
                    id: p.id,
                });
            }
        }
    }
    out
}

/// Вычесть отрезок `[lo, hi)` из набора непересекающихся отрезков.
fn subtract_span(spans: &[(f32, f32)], lo: f32, hi: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(spans.len() + 1);
    for &(a, b) in spans {
        if hi <= a || lo >= b {
            out.push((a, b)); // не пересекаются
            continue;
        }
        if lo > a {
            out.push((a, lo));
        }
        if hi < b {
            out.push((hi, b));
        }
    }
    out
}

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

    // ---- Видимые кромки (фаза G2) ---------------------------------------

    fn screen() -> Rect {
        Rect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    /// Окно с явной высотой: для перекрытий важно, докуда окно достаёт вниз.
    fn win(x: f32, y: f32, w: f32, h: f32, id: u64) -> Platform {
        Platform {
            rect: Rect::new(x, y, w, h),
            id,
        }
    }

    /// Окно, целиком накрытое окном сверху, платформ не даёт: именно так
    /// питомец и оказывался «стоящим в воздухе» посреди чужого окна.
    #[test]
    fn covered_window_gives_no_ledge() {
        let stack = vec![
            win(0.0, 100.0, 1920.0, 900.0, 1),  // сверху, почти во весь экран
            win(300.0, 500.0, 600.0, 400.0, 2), // под ним
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        assert_eq!(vis.len(), 1, "видна только кромка верхнего окна: {vis:?}");
        assert_eq!(vis[0].id, 1);
    }

    /// Частичное перекрытие: кромка режется на видимые куски.
    #[test]
    fn partly_covered_ledge_splits_into_visible_spans() {
        let stack = vec![
            win(500.0, 100.0, 300.0, 900.0, 1), // накрывает середину кромки
            win(300.0, 500.0, 900.0, 400.0, 2),
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        let lower: Vec<_> = vis.iter().filter(|p| p.id == 2).collect();
        assert_eq!(lower.len(), 2, "две видимые части: {vis:?}");
        assert_eq!((lower[0].rect.x, lower[0].rect.right()), (300.0, 500.0));
        assert_eq!((lower[1].rect.x, lower[1].rect.right()), (800.0, 1200.0));
    }

    /// Окно сверху, но НИЖЕ кромки (его верх ниже) — не мешает: кромка видна.
    #[test]
    fn window_below_the_ledge_does_not_cover_it() {
        let stack = vec![
            win(0.0, 700.0, 1920.0, 300.0, 1), // выше по стекингу, ниже по экрану
            win(300.0, 500.0, 600.0, 400.0, 2),
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        assert!(vis.iter().any(|p| p.id == 2), "кромка не накрыта: {vis:?}");
    }

    /// Узкие щели между окнами платформами не становятся.
    #[test]
    fn narrow_gaps_are_dropped() {
        let stack = vec![
            win(0.0, 100.0, 500.0, 900.0, 1),
            win(510.0, 100.0, 500.0, 900.0, 2), // щель 10 px между ними
            win(0.0, 500.0, 1920.0, 400.0, 3),
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        let bottom: Vec<_> = vis.iter().filter(|p| p.id == 3).collect();
        assert!(
            bottom.iter().all(|p| p.rect.w >= MIN_LEDGE),
            "щели отброшены: {bottom:?}"
        );
        assert!(
            !bottom.iter().any(|p| (p.rect.x - 500.0).abs() < 1.0),
            "щели 500..510 быть не должно: {bottom:?}"
        );
    }

    /// Кромки за пределами экрана обрезаются, а совсем чужие — выбрасываются.
    #[test]
    fn ledges_are_clipped_to_the_screen() {
        let stack = vec![
            win(-400.0, 300.0, 900.0, 400.0, 1), // торчит слева за экран
            win(0.0, -50.0, 800.0, 400.0, 2),    // кромка над экраном
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        assert_eq!(vis.len(), 1);
        assert_eq!((vis[0].rect.x, vis[0].rect.right()), (0.0, 500.0));
    }

    /// Видимые кромки годятся как есть для support_below: питомец на
    /// накрытом окне опоры не находит и падает на землю.
    #[test]
    fn support_uses_visible_ledges_only() {
        let stack = vec![
            win(0.0, 100.0, 1920.0, 900.0, 1),
            win(300.0, 500.0, 600.0, 400.0, 2),
        ];
        let mut w = World::new(screen());
        w.platforms = visible_ledges(&stack, screen(), MIN_LEDGE, 0.0);
        // Точка под верхним окном, но над «накрытой» кромкой второго.
        assert_eq!(support_below(&w, 600.0, 300.0), w.ground_y());
    }

    /// Кромка максимизированного окна (y = верх экрана) платформой не
    /// становится: стоя на ней, питомец был бы телом за краем экрана.
    #[test]
    fn ledge_without_head_room_is_dropped() {
        let stack = vec![
            win(0.0, 0.0, 1920.0, 1080.0, 1), // максимизированное
            win(200.0, 300.0, 600.0, 400.0, 2),
        ];
        let vis = visible_ledges(&stack, screen(), MIN_LEDGE, 64.0);
        assert!(vis.is_empty(), "стоять негде: {vis:?}");
        // Без требования места кромка бы нашлась — значит отсекает именно оно.
        assert_eq!(visible_ledges(&stack, screen(), MIN_LEDGE, 0.0).len(), 1);
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
