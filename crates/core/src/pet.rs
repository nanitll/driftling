//! Питомец и мир, в котором он живёт. Точка входа симуляции — `Pet::tick`.

use crate::behavior::{next_state_after, BehaviorConfig, PetState};
use crate::geometry::{Rect, Vec2};
use crate::physics::{support_below, Platform, STEP_SNAP, SUPPORT_TOL};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

impl Direction {
    pub fn sign(self) -> f32 {
        match self {
            Direction::Left => -1.0,
            Direction::Right => 1.0,
        }
    }

    pub fn flip(self) -> Self {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }
}

/// Мир: один экран, земля — нижняя кромка (или верх панели, D5),
/// плюс платформы из worldsense — верхние кромки окон (D4).
/// Демон пересобирает `platforms` (и при необходимости
/// `ground_y_override`) на каждый снапшот worldsense.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub screen: Rect,
    /// Верх панели (exclusive-зона): переопределяет землю.
    pub ground_y_override: Option<f32>,
    /// Верхние кромки окон, по которым можно ходить.
    pub platforms: Vec<Platform>,
}

impl World {
    /// Мир без платформ и без панели — как в M0.
    pub fn new(screen: Rect) -> Self {
        Self {
            screen,
            ground_y_override: None,
            platforms: Vec::new(),
        }
    }

    pub fn ground_y(&self) -> f32 {
        self.ground_y_override
            .unwrap_or_else(|| self.screen.bottom())
    }
}

/// Желаемый темп симуляции (энергобюджет ТЗ §7): ядро не знает о платформе,
/// демон мапит SimPace на таймер бэкенда (см. driftling_platform::Pace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimPace {
    /// Движение/падение/drag — нужен частый тик.
    Active,
    /// Спокойный idle/оглушение — редкий тик.
    Calm,
    /// Сон — тик раз в секунду.
    Drowsy,
}

/// События указателя, которые платформа передаёт в симуляцию.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    /// Нажатие по питомцу (позиция курсора).
    Press(Vec2),
    /// Движение при зажатой кнопке.
    Motion(Vec2),
    /// Отпускание кнопки.
    Release(Vec2),
}

#[derive(Debug, Clone)]
pub struct Pet {
    /// Позиция опорной точки: центр нижней кромки спрайта.
    pub pos: Vec2,
    pub vel: Vec2,
    pub facing: Direction,
    pub state: PetState,
    /// Сколько питомец уже находится в текущем состоянии, сек.
    pub state_time: f32,
    /// Сколько осталось до следующего решения машины поведения, сек.
    pub state_left: f32,
    /// Размер спрайта в логических пикселях (квадрат).
    pub size: f32,
    cfg: BehaviorConfig,
    rng: fastrand::Rng,
    /// Смещение точки хвата при перетаскивании.
    drag_offset: Vec2,
    /// Недавние позиции курсора для скорости броска.
    drag_history: [(f32, Vec2); 4],
    /// Точка нажатия, пока не решено «клик или drag» (ТД-24): кнопка зажата,
    /// но порог движения ещё не пройден. None — кнопка не зажата.
    pressed_at: Option<Vec2>,
    /// «Только на земле» (фаза B6): машина поведения не входит в Walk.
    /// Включается демоном на стадии яйца — яйцо не ходит.
    grounded_only: bool,
    /// Последний Release оказался кликом (нажатие без захвата, ТД-24);
    /// снимается чтением [`Pet::take_click`].
    clicked: bool,
}

/// Порог движения курсора, после которого нажатие становится захватом,
/// а не кликом (ТД-24), логические пиксели.
const DRAG_THRESHOLD: f32 = 4.0;

impl Pet {
    pub fn new(pos: Vec2, size: f32, cfg: BehaviorConfig, seed: u64) -> Self {
        let mut rng = fastrand::Rng::with_seed(seed);
        let state_left = 0.5 + rng.f32();
        Self {
            pos,
            vel: Vec2::default(),
            facing: Direction::Right,
            state: PetState::Falling,
            state_time: 0.0,
            state_left,
            size,
            cfg,
            rng,
            drag_offset: Vec2::default(),
            drag_history: [(0.0, Vec2::default()); 4],
            pressed_at: None,
            grounded_only: false,
            clicked: false,
        }
    }

    pub fn config(&self) -> &BehaviorConfig {
        &self.cfg
    }

    /// Применить новые настройки к живому питомцу: поведение и размер
    /// меняются, состояние (позиция, текущее занятие) сохраняется.
    pub fn apply_config(&mut self, cfg: BehaviorConfig, size: f32) {
        self.cfg = cfg;
        self.size = size;
    }

    /// Режим «только на земле» (фаза B6, стадия яйца): пока включён, машина
    /// поведения никогда не входит в Walk — питомец стоит на месте (Idle/
    /// Sleep). Уже идущая прогулка прерывается сразу. Падение и drag не
    /// трогаем: физика (уронили яйцо — оно падает) важнее запрета ходьбы.
    pub fn set_grounded_only(&mut self, grounded: bool) {
        self.grounded_only = grounded;
        if grounded && self.state == PetState::Walk {
            self.enter(PetState::Idle);
            self.state_left = self.roll(self.cfg.idle_range);
        }
    }

    /// Принудительно уложить спать (уход, фаза B5: команда «Уложить спать»).
    /// Питомец засыпает сразу на полную длительность `cfg.sleep_range.1`
    /// (максимум из настроек сна — явная команда даёт самый долгий сон).
    /// В Dragged/Falling не действует (сон в воздухе ломал бы физику) —
    /// возвращает false. Повторный вызов во сне перевзводит таймер заново.
    pub fn force_sleep(&mut self) -> bool {
        if matches!(self.state, PetState::Dragged | PetState::Falling) {
            return false;
        }
        self.state = PetState::Sleep;
        self.state_time = 0.0;
        self.state_left = self.cfg.sleep_range.1;
        true
    }

    /// Был ли с прошлого вызова потреблённый клик по питомцу (нажатие и
    /// отпускание без прохождения порога захвата, ТД-24)? Флаг снимается
    /// чтением — демон превращает его в поглаживание (Petted, фаза B5).
    pub fn take_click(&mut self) -> bool {
        core::mem::take(&mut self.clicked)
    }

    /// Прямоугольник спрайта (для рендера и input region).
    pub fn bounds(&self) -> Rect {
        Rect::new(
            self.pos.x - self.size / 2.0,
            self.pos.y - self.size,
            self.size,
            self.size,
        )
    }

    /// Желаемый темп тика: адаптивный таймер бэкенда (ТД-3) спрашивает у
    /// симуляции, как часто её надо будить.
    pub fn pace(&self) -> SimPace {
        match self.state {
            PetState::Falling | PetState::Dragged | PetState::Walk => SimPace::Active,
            PetState::Idle | PetState::Landing => SimPace::Calm,
            PetState::Sleep => SimPace::Drowsy,
        }
    }

    /// Один шаг симуляции. `dt` — секунды с прошлого шага (клампится).
    ///
    /// Кламп 1.5 с — под адаптивный таймер (ТД-3): спящий питомец тикает
    /// ~1 Гц, и его таймеры должны идти в реальном темпе, а не в 10 раз
    /// медленнее. Больший dt (сон компоситора, лаг) честно обрезаем.
    pub fn tick(&mut self, world: &World, dt: f32) {
        let dt = dt.clamp(0.0, 1.5);
        self.state_time += dt;

        match self.state {
            PetState::Dragged => {
                // Позицию ведёт указатель (см. pointer()); физика выключена.
            }
            PetState::Falling => {
                // Падение при частом тике не случается с большим dt, но
                // подстраховываем интегрирование: подшаги ≤0.05 с, иначе
                // редкий тик протыкает землю и завышает скорость удара.
                let mut left = dt;
                while left > 0.0 && self.state == PetState::Falling {
                    let step = left.min(0.05);
                    left -= step;
                    // Опору ищем от ног ДО подшага: быстрый подшаг не должен
                    // протыкать кромку окна насквозь. Кромки выше исходной
                    // позиции не считаются — на окно садимся только сверху.
                    let feet_before = self.pos.y;
                    self.vel.y += self.cfg.gravity * step;
                    self.pos = self.pos + self.vel * step;
                    self.clamp_horizontal(world);
                    let support = support_below(world, self.pos.x, feet_before);
                    if self.pos.y >= support {
                        self.pos.y = support;
                        let impact = self.vel.y;
                        self.vel = Vec2::default();
                        self.enter(if impact > self.cfg.hard_landing_speed {
                            PetState::Landing
                        } else {
                            PetState::Idle
                        });
                        self.state_left = if self.state == PetState::Landing {
                            self.cfg.landing_time
                        } else {
                            1.0 + self.rng.f32()
                        };
                    }
                }
            }
            PetState::Walk => {
                self.pos.x += self.cfg.walk_speed * self.facing.sign() * dt;
                let b = self.bounds();
                if b.x <= world.screen.x {
                    self.pos.x = world.screen.x + self.size / 2.0;
                    self.facing = Direction::Right;
                } else if b.right() >= world.screen.right() {
                    self.pos.x = world.screen.right() - self.size / 2.0;
                    self.facing = Direction::Left;
                }
                // Пол под ногами на новой позиции: перепад в пределах
                // STEP_SNAP перешагиваем («ступеньки» окон), обрыв вниз
                // больше порога — сошли с кромки, падаем.
                let feet = self.pos.y;
                let support = support_below(world, self.pos.x, feet - STEP_SNAP);
                if support - feet > STEP_SNAP {
                    self.vel = Vec2::new(self.cfg.walk_speed * self.facing.sign(), 0.0);
                    self.enter(PetState::Falling);
                    return;
                }
                self.pos.y = support;
                self.advance_timer();
            }
            PetState::Idle | PetState::Sleep | PetState::Landing => {
                self.advance_timer();
            }
        }
    }

    /// Мир изменился (демон обновил платформы/панель по снапшоту worldsense).
    /// Проверяем опору под ногами стоящего питомца (Idle/Walk/Sleep/Landing):
    /// - опора уехала по вертикали не дальше [`SUPPORT_TOL`] — догоняем снапом
    ///   (окно чуть сдвинули — питомец едет вместе с кромкой);
    /// - опоры в пределах допуска больше нет (окно закрыли/свернули/увезли) —
    ///   падаем, даже во сне: приземление выведет в Idle/Landing, т. е.
    ///   Falling будит питомца.
    ///
    /// Falling и Dragged не трогаем: в воздухе опора не нужна, а позицию
    /// в drag ведёт указатель (отпустили над окном — питомец упадёт на него
    /// обычной физикой Falling).
    ///
    /// Вежливость к fullscreen (D5) — целиком на стороне демона: при
    /// `fullscreen_active` он прячет сцену, продолжая тикать симуляцию;
    /// ядру для этого ничего не нужно — `tick` работает и «за кадром».
    pub fn world_changed(&mut self, world: &World) {
        if matches!(self.state, PetState::Falling | PetState::Dragged) {
            return;
        }
        let feet = self.pos.y;
        let support = support_below(world, self.pos.x, feet - SUPPORT_TOL);
        if (support - feet).abs() <= SUPPORT_TOL {
            self.pos.y = support;
        } else {
            self.vel = Vec2::default();
            self.enter(PetState::Falling);
        }
    }

    /// Обработка событий указателя. Возвращает true, если событие потреблено.
    pub fn pointer(&mut self, world: &World, ev: PointerEvent, now: f32) -> bool {
        match ev {
            PointerEvent::Press(p) => {
                if !self.bounds().contains(p) {
                    return false;
                }
                // Захват начнётся только после порога движения (ТД-24):
                // пока лишь запоминаем точку нажатия.
                self.pressed_at = Some(p);
                self.drag_history = [(now, p); 4];
                true
            }
            PointerEvent::Motion(p) => {
                if self.state != PetState::Dragged {
                    let Some(origin) = self.pressed_at else {
                        return false;
                    };
                    if (p.x - origin.x).hypot(p.y - origin.y) < DRAG_THRESHOLD {
                        // Дрожание в пределах порога — всё ещё клик.
                        return true;
                    }
                    // Порог пройден — это захват. Смещение от текущей позиции
                    // питомца, чтобы он не прыгал под курсор.
                    self.drag_offset = Vec2::new(self.pos.x - p.x, self.pos.y - p.y);
                    self.enter(PetState::Dragged);
                }
                self.drag_history.rotate_left(1);
                self.drag_history[3] = (now, p);
                self.pos = p + self.drag_offset;
                // Не даём утащить за экран.
                self.pos.x = self.pos.x.clamp(world.screen.x, world.screen.right());
                self.pos.y = self.pos.y.clamp(world.screen.y, world.ground_y());
                true
            }
            PointerEvent::Release(_) => {
                let was_pressed = self.pressed_at.take().is_some();
                if self.state != PetState::Dragged {
                    // Клик без захвата: потребляем и запоминаем — демон
                    // прочитает флаг через take_click (поглаживание, B5).
                    self.clicked |= was_pressed;
                    return was_pressed;
                }
                // Скорость броска — по финальному «флику»: берём самую раннюю
                // точку истории не старше ~120 мс, чтобы медленное таскание
                // до рывка не гасило скорость.
                let (t1, p1) = self.drag_history[3];
                let (t0, p0) = self
                    .drag_history
                    .iter()
                    .copied()
                    .find(|(t, _)| t1 - t <= 0.12)
                    .unwrap_or(self.drag_history[2]);
                let span = (t1 - t0).max(1e-3);
                self.vel = Vec2::new((p1.x - p0.x) / span, (p1.y - p0.y) / span);
                self.enter(PetState::Falling);
                true
            }
        }
    }

    fn advance_timer(&mut self) {
        if self.state_time >= self.state_left {
            let (mut next, mut dur) = next_state_after(self.state, &self.cfg, &mut self.rng);
            if self.grounded_only && next == PetState::Walk {
                // Яйцо не ходит (B6): решение «гулять» заменяется на Idle.
                next = PetState::Idle;
                dur = self.roll(self.cfg.idle_range);
            }
            if next == PetState::Walk && self.rng.bool() {
                self.facing = self.facing.flip();
            }
            self.enter(next);
            self.state_left = dur;
        }
    }

    /// Случайная длительность из диапазона (lo, hi).
    fn roll(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.rng.f32()
    }

    fn enter(&mut self, next: PetState) {
        if self.state != next {
            self.state = next;
            self.state_time = 0.0;
        }
    }

    fn clamp_horizontal(&mut self, world: &World) {
        self.pos.x = self.pos.x.clamp(
            world.screen.x + self.size / 2.0,
            world.screen.right() - self.size / 2.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        World::new(Rect::new(0.0, 0.0, 1920.0, 1080.0))
    }

    /// Мир с платформами-окнами (кромки задаются (x, top, ширина)).
    fn world_with(edges: &[(f32, f32, f32)]) -> World {
        let mut w = world();
        w.platforms = edges
            .iter()
            .enumerate()
            .map(|(i, &(x, top, width))| Platform {
                rect: Rect::new(x, top, width, 400.0),
                id: i as u64,
            })
            .collect();
        w
    }

    fn pet() -> Pet {
        Pet::new(Vec2::new(960.0, 100.0), 96.0, BehaviorConfig::default(), 42)
    }

    #[test]
    fn falls_and_lands_on_ground() {
        let w = world();
        let mut p = pet();
        assert_eq!(p.state, PetState::Falling);
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
        assert!(matches!(
            p.state,
            PetState::Idle | PetState::Walk | PetState::Sleep
        ));
    }

    #[test]
    fn walk_turns_at_screen_edge() {
        let w = world();
        let mut p = pet();
        // Приземлить и заставить идти влево от левого края.
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        p.pos.x = 50.0;
        p.facing = Direction::Left;
        p.state = PetState::Walk;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        for _ in 0..300 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.facing, Direction::Right);
        assert!(p.bounds().x >= 0.0);
    }

    #[test]
    fn drag_and_throw() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        assert_ne!(p.state, PetState::Dragged, "до порога движения — не захват");
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(500.0, 300.0)), 0.1));
        assert_eq!(p.state, PetState::Dragged);
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(700.0, 280.0)), 0.2));
        assert!(p.pointer(&w, PointerEvent::Release(Vec2::new(700.0, 280.0)), 0.2));
        assert_eq!(p.state, PetState::Falling);
        assert!(p.vel.x > 0.0, "бросок вправо должен дать положительную vx");
        // Падение после броска завершается на земле.
        for _ in 0..1200 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
    }

    #[test]
    fn press_outside_is_ignored() {
        let w = world();
        let mut p = pet();
        assert!(!p.pointer(&w, PointerEvent::Press(Vec2::new(5.0, 5.0)), 0.0));
    }

    /// ТД-24: клик (нажал-отпустил, дрожание в пределах порога) — не захват.
    #[test]
    fn click_is_not_a_grab() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let state = p.state;
        let pos = p.pos;
        let press = Vec2::new(p.pos.x, p.pos.y - 10.0);
        let jitter = Vec2::new(press.x + 1.0, press.y - 1.0);
        assert!(p.pointer(&w, PointerEvent::Press(press), 0.0));
        assert!(p.pointer(&w, PointerEvent::Motion(jitter), 0.05));
        assert!(p.pointer(&w, PointerEvent::Release(jitter), 0.1));
        assert_eq!(p.state, state, "клик не должен менять состояние");
        assert_eq!(
            (p.pos.x, p.pos.y),
            (pos.x, pos.y),
            "клик не двигает питомца"
        );
        // Отпустили — дальнейший Motion питомца не касается.
        assert!(!p.pointer(&w, PointerEvent::Motion(Vec2::new(500.0, 300.0)), 0.2));
    }

    /// ТД-3: при редком тике (~1 Гц во сне) таймеры состояния должны идти
    /// в реальном темпе — старый кламп 0.1 растягивал сон в 10 раз.
    #[test]
    fn sleep_timer_is_exact_at_one_second_ticks() {
        let w = world();
        let mut p = pet();
        p.pos.y = w.ground_y();
        p.vel = Vec2::default();
        p.state = PetState::Sleep;
        p.state_time = 0.0;
        p.state_left = 20.0;
        for _ in 0..19 {
            p.tick(&w, 1.0);
        }
        assert_eq!(p.state, PetState::Sleep, "19 секунд из 20 — ещё спит");
        p.tick(&w, 1.0);
        assert_eq!(p.state, PetState::Idle, "ровно на 20-й секунде проснулся");
    }

    /// dt больше клампа честно обрезается до 1.5 с.
    #[test]
    fn dt_above_clamp_is_cut() {
        let w = world();
        let mut p = pet();
        p.pos.y = w.ground_y();
        p.state = PetState::Sleep;
        p.state_time = 0.0;
        p.state_left = 10.0;
        p.tick(&w, 100.0);
        assert_eq!(p.state, PetState::Sleep);
        assert!((p.state_time - 1.5).abs() < 1e-6);
    }

    /// Интегрирование падения при dt=1.5 идёт подшагами и совпадает с
    /// мелкошаговой референс-симуляцией: земля не протыкается, скорость
    /// удара не завышается.
    #[test]
    fn fall_substepping_matches_fine_steps() {
        let w = world();
        // Два одинаковых питомца: один тикает крупно, другой мелко.
        let mut coarse = pet();
        let mut fine = pet();
        coarse.tick(&w, 1.5);
        for _ in 0..30 {
            fine.tick(&w, 0.05);
        }
        // За 1.5 с при g=1800 оба обязаны долететь до земли без пролёта.
        assert_eq!(coarse.pos.y, w.ground_y());
        assert_eq!(fine.pos.y, w.ground_y());
        assert_eq!(coarse.state, fine.state);
        assert!(
            (coarse.pos.x - fine.pos.x).abs() < 1.0,
            "траектории разошлись: {} vs {}",
            coarse.pos.x,
            fine.pos.x
        );
    }

    /// B6: в режиме «только на земле» (стадия яйца) машина поведения
    /// никогда не входит в Walk, а текущая прогулка прерывается сразу.
    #[test]
    fn grounded_only_never_walks() {
        let w = world();
        let mut p = pet();
        p.set_grounded_only(true);
        // Долгая жизнь с частым тиком: ни одного Walk за всё время.
        for _ in 0..20_000 {
            p.tick(&w, 1.0 / 30.0);
            assert_ne!(p.state, PetState::Walk, "яйцо не ходит");
        }
        assert_eq!(p.pos.y, w.ground_y(), "и стоит на земле");

        // Прогулка в момент включения режима прерывается немедленно.
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        p.state = PetState::Walk;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        p.set_grounded_only(true);
        assert_eq!(p.state, PetState::Idle);
        assert!(p.state_left.is_finite());
    }

    /// B5: force_sleep укладывает немедленно на максимум sleep_range,
    /// но не действует в воздухе (Falling/Dragged).
    #[test]
    fn force_sleep_sleeps_now_for_cfg_max() {
        let w = world();
        let mut p = pet();
        assert!(!p.force_sleep(), "в падении сон не форсируется");
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert!(p.force_sleep());
        assert_eq!(p.state, PetState::Sleep);
        assert_eq!(p.state_left, p.cfg.sleep_range.1);
        assert_eq!(p.state_time, 0.0);

        // Во время drag тоже не действует.
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(500.0, 300.0)), 0.1));
        assert_eq!(p.state, PetState::Dragged);
        assert!(!p.force_sleep());
        assert_eq!(p.state, PetState::Dragged);
    }

    /// B5: клик (нажал-отпустил без захвата) взводит флаг take_click ровно
    /// один раз; drag с броском флага не взводит.
    #[test]
    fn take_click_reports_click_once() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert!(!p.take_click(), "до клика флага нет");
        let press = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(press), 0.0));
        assert!(p.pointer(&w, PointerEvent::Release(press), 0.1));
        assert!(p.take_click(), "клик замечен");
        assert!(!p.take_click(), "флаг снимается чтением");

        // Полноценный drag кликом не считается.
        assert!(p.pointer(&w, PointerEvent::Press(press), 0.2));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(500.0, 300.0)), 0.3));
        assert!(p.pointer(&w, PointerEvent::Release(Vec2::new(500.0, 300.0)), 0.4));
        assert!(!p.take_click(), "захват — не поглаживание");
    }

    #[test]
    fn pace_maps_states_to_sim_pace() {
        let mut p = pet();
        for (state, pace) in [
            (PetState::Falling, SimPace::Active),
            (PetState::Dragged, SimPace::Active),
            (PetState::Walk, SimPace::Active),
            (PetState::Idle, SimPace::Calm),
            (PetState::Landing, SimPace::Calm),
            (PetState::Sleep, SimPace::Drowsy),
        ] {
            p.state = state;
            assert_eq!(p.pace(), pace, "{state:?}");
        }
    }

    // ---- Фаза D: окна-рельеф -------------------------------------------

    /// Падение заканчивается на верхней кромке окна, а не только на земле.
    #[test]
    fn falling_lands_on_window_top() {
        // Кромка во всю ширину, чтобы питомец не ушёл с неё за время теста.
        let w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, 500.0, "стоит на кромке окна");
        assert_ne!(p.state, PetState::Falling);
    }

    /// Сошёл с кромки при ходьбе — падает и приземляется на землю.
    #[test]
    fn walking_off_edge_falls() {
        let w = world_with(&[(100.0, 500.0, 400.0)]);
        let mut p = pet();
        p.pos = Vec2::new(450.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Walk;
        p.facing = Direction::Right;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        let mut fell = false;
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
            fell |= p.state == PetState::Falling;
        }
        assert!(fell, "за кромкой окна должен начаться Falling");
        assert_eq!(p.pos.y, w.ground_y(), "долетел до земли");
    }

    /// Перепад кромок в пределах STEP_SNAP перешагивается — «ступеньки» окон.
    #[test]
    fn small_step_up_is_snapped_while_walking() {
        let w = world_with(&[(100.0, 500.0, 200.0), (300.0, 492.0, 400.0)]);
        let mut p = pet();
        p.pos = Vec2::new(250.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Walk;
        p.facing = Direction::Right;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        for _ in 0..180 {
            p.tick(&w, 1.0 / 60.0);
            assert_ne!(p.state, PetState::Falling, "ступенька в 8px — не обрыв");
        }
        assert!(p.pos.x > 300.0, "дошёл до второго окна");
        assert_eq!(p.pos.y, 492.0, "поднялся на ступеньку");
        assert_eq!(p.state, PetState::Walk);
    }

    /// Ступенька вниз больше STEP_SNAP — падение с приземлением на нижнюю кромку.
    #[test]
    fn big_step_down_falls_onto_lower_edge() {
        let w = world_with(&[(100.0, 492.0, 200.0), (300.0, 522.0, 600.0)]);
        let mut p = pet();
        p.pos = Vec2::new(250.0, 492.0);
        p.vel = Vec2::default();
        p.state = PetState::Walk;
        p.facing = Direction::Right;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        let mut fell = false;
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
            fell |= p.state == PetState::Falling;
            if fell && p.state != PetState::Falling {
                // Проверяем сразу в момент приземления: дальше питомец
                // может снова уйти гулять и свалиться с узкой кромки.
                break;
            }
        }
        assert!(fell, "перепад в 30px — обрыв");
        assert_eq!(p.pos.y, 522.0, "приземлился на нижнюю кромку");
    }

    /// Окно закрыли под стоящим питомцем — world_changed роняет его на землю.
    #[test]
    fn window_vanish_under_idle_pet_falls() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        p.pos = Vec2::new(960.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Idle;
        p.state_time = 0.0;
        p.state_left = 1000.0;
        w.platforms.clear();
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Falling);
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// Окно закрыли под спящим — падает даже во сне, и падение его будит.
    #[test]
    fn window_vanish_wakes_sleeping_pet() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        p.pos = Vec2::new(960.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Sleep;
        p.state_time = 0.0;
        p.state_left = 1000.0;
        w.platforms.clear();
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Falling, "сон прерван падением");
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, w.ground_y());
        assert_ne!(p.state, PetState::Sleep, "после падения не спит");
    }

    /// Окно чуть сдвинули по вертикали (≤ SUPPORT_TOL) — питомец едет
    /// вместе с кромкой, не просыпаясь.
    #[test]
    fn small_window_move_snaps_without_waking() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        p.pos = Vec2::new(960.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Sleep;
        p.state_time = 3.0;
        p.state_left = 1000.0;
        w.platforms[0].rect.y = 503.0;
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Sleep, "снап не будит");
        assert_eq!(p.pos.y, 503.0, "ноги догнали кромку");
        assert_eq!(p.state_time, 3.0, "таймер состояния не сброшен");
    }

    /// Окно резко уехало вверх — опоры под ногами больше нет, падаем.
    #[test]
    fn window_moved_far_up_drops_pet() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        p.pos = Vec2::new(960.0, 500.0);
        p.vel = Vec2::default();
        p.state = PetState::Idle;
        p.state_time = 0.0;
        p.state_left = 1000.0;
        w.platforms[0].rect.y = 470.0;
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Falling);
    }

    /// Верх панели (D5, exclusive-зона) — пол: падение заканчивается на нём.
    #[test]
    fn panel_override_acts_as_ground() {
        let mut w = world();
        w.ground_y_override = Some(1040.0);
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, 1040.0, "земля — верх панели");
    }

    /// Питомца на земле окна ВЫШЕ него не касаются: появление окна над
    /// головой ничего не меняет.
    #[test]
    fn ground_pet_unaffected_by_windows_above() {
        let mut w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let (state, y) = (p.state, p.pos.y);
        assert_eq!(y, w.ground_y());
        w.platforms = vec![Platform {
            rect: Rect::new(0.0, 500.0, 1920.0, 400.0),
            id: 7,
        }];
        p.world_changed(&w);
        assert_eq!(p.state, state, "окно над головой не меняет состояние");
        assert_eq!(p.pos.y, y, "и не двигает питомца");
        for _ in 0..120 {
            p.tick(&w, 1.0 / 60.0);
            assert_eq!(p.pos.y, w.ground_y(), "ходьба остаётся на земле");
        }
    }

    /// Отпустили питомца над окном — он падает и приземляется на кромку.
    #[test]
    fn release_over_window_lands_on_it() {
        let w = world_with(&[(300.0, 500.0, 600.0)]);
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        // На всякий случай вернём на землю (мог приземлиться и на кромку).
        p.pos = Vec2::new(960.0, w.ground_y());
        p.state = PetState::Idle;
        p.state_left = 1000.0;
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        // Захват и перенос над окно; последние точки совпадают — без флика.
        assert!(p.pointer(
            &w,
            PointerEvent::Motion(Vec2::new(grab.x + 5.0, grab.y)),
            0.05
        ));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(600.0, 300.0)), 0.1));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(600.0, 300.0)), 0.3));
        assert!(p.pointer(&w, PointerEvent::Release(Vec2::new(600.0, 300.0)), 0.3));
        assert_eq!(p.state, PetState::Falling);
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.pos.y, 500.0, "упал на кромку окна под точкой отпускания");
    }

    /// Dragged не трогается world_changed: позицию ведёт указатель.
    #[test]
    fn dragged_ignores_world_changes() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(600.0, 300.0)), 0.1));
        assert_eq!(p.state, PetState::Dragged);
        let pos = p.pos;
        w.platforms.clear();
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Dragged);
        assert_eq!((p.pos.x, p.pos.y), (pos.x, pos.y));
    }
}
