//! Питомец и мир, в котором он живёт. Точка входа симуляции — `Pet::tick`.

use crate::behavior::{
    next_idle_action, next_state_after, next_state_off_floor, BehaviorConfig, IdleAction, PetState,
};
use crate::geometry::{Rect, Vec2};
use crate::physics::{support_below, Orient, Platform, Surface, STEP_SNAP, SUPPORT_TOL};

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
    /// Опорная точка: центр той кромки спрайта, которой питомец касается
    /// поверхности (для пола — центр нижней кромки, как было до фазы G).
    pub pos: Vec2,
    pub vel: Vec2,
    pub facing: Direction,
    /// Поверхность, к которой питомец прижат (фаза G). В воздухе
    /// (Falling/Dragged) всегда [`Surface::Floor`]: опорная точка — «ноги».
    pub surface: Surface,
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
    /// Включается демоном на стадии яйца — яйцо не ходит и не лазает.
    grounded_only: bool,
    /// Последний Release оказался кликом (нажатие без захвата, ТД-24);
    /// снимается чтением [`Pet::take_click`].
    clicked: bool,
    /// Текущее мелкое занятие в Idle (фаза G).
    idle_action: IdleAction,
    /// Сколько занятие уже идёт, сек (фаза анимации кадра).
    action_time: f32,
    /// Сколько осталось до смены занятия, сек.
    action_left: f32,
    /// Уснуть сразу после приземления: команда «Уложить спать», отданная
    /// на стене или потолке, сначала отцепляет питомца (фаза G).
    sleep_on_land: bool,
    /// Текущая прогулка — «поход к стене» (фаза G): питомец идёт до края
    /// экрана не сворачивая и лезет наверх. Снимается на выходе из Walk.
    wall_trip: bool,
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
            surface: Surface::Floor,
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
            idle_action: IdleAction::Stand,
            action_time: 0.0,
            action_left: 1.0,
            sleep_on_land: false,
            wall_trip: false,
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
        if grounded {
            self.wall_trip = false;
        }
        if grounded && matches!(self.state, PetState::Walk | PetState::Climb) {
            // Со стены яйцо честно падает: висеть ему тоже не положено.
            if self.surface != Surface::Floor {
                self.detach();
                return;
            }
            self.enter(PetState::Idle);
            self.state_left = self.roll(self.cfg.idle_range);
        }
    }

    /// Принудительно уложить спать (уход, фаза B5: команда «Уложить спать»).
    /// Питомец засыпает сразу на полную длительность `cfg.sleep_range.1`
    /// (максимум из настроек сна — явная команда даёт самый долгий сон).
    /// В Dragged/Falling/Bonk не действует (сон в воздухе ломал бы физику) —
    /// возвращает false. Повторный вызов во сне перевзводит таймер заново.
    ///
    /// Со стены или потолка (фаза G) спать нельзя: питомец отцепляется и
    /// засыпает, как только приземлится — команда всё равно принята (true).
    pub fn force_sleep(&mut self) -> bool {
        if matches!(
            self.state,
            PetState::Dragged | PetState::Falling | PetState::Bonk
        ) {
            return false;
        }
        if self.surface != Surface::Floor {
            self.sleep_on_land = true;
            self.detach();
            return true;
        }
        self.state = PetState::Sleep;
        self.state_time = 0.0;
        self.state_left = self.cfg.sleep_range.1;
        self.idle_action = IdleAction::Stand;
        true
    }

    /// Был ли с прошлого вызова потреблённый клик по питомцу (нажатие и
    /// отпускание без прохождения порога захвата, ТД-24)? Флаг снимается
    /// чтением — демон превращает его в поглаживание (Petted, фаза B5).
    pub fn take_click(&mut self) -> bool {
        core::mem::take(&mut self.clicked)
    }

    /// Прямоугольник спрайта (для рендера и input region) — с учётом
    /// поверхности: на стене питомец лежит поперёк, под потолком висит.
    pub fn bounds(&self) -> Rect {
        self.surface.bounds(self.pos, self.size)
    }

    /// Ориентация кадра для рендера: поворот под поверхность + зеркало по
    /// направлению взгляда (кадры пака нарисованы мордой вправо, ногами вниз).
    pub fn orient(&self) -> Orient {
        self.surface.orient(self.facing)
    }

    /// Текущее мелкое занятие в Idle (фаза G) — для выбора кадра.
    pub fn idle_action(&self) -> IdleAction {
        self.idle_action
    }

    /// Время для фазы анимации: у мелкого занятия — своё, иначе время
    /// в состоянии.
    pub fn anim_time(&self) -> f32 {
        if self.idle_action == IdleAction::Stand {
            self.state_time
        } else {
            self.action_time
        }
    }

    /// Желаемый темп тика: адаптивный таймер бэкенда (ТД-3) спрашивает у
    /// симуляции, как часто её надо будить.
    pub fn pace(&self) -> SimPace {
        match self.state {
            PetState::Falling
            | PetState::Dragged
            | PetState::Walk
            | PetState::Climb
            | PetState::Bonk => SimPace::Active,
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
        self.advance_fidget(dt);

        match self.state {
            PetState::Dragged => {
                // Позицию ведёт указатель (см. pointer()); физика выключена.
            }
            PetState::Falling => self.tick_falling(world, dt),
            PetState::Walk => self.tick_walk(world, dt),
            PetState::Climb => self.tick_climb(world, dt),
            PetState::Bonk => {
                // Шишка о потолок: короткое оглушение в воздухе, дальше вниз.
                if self.state_time >= self.state_left {
                    self.vel = Vec2::new(self.vel.x * 0.3, 0.0);
                    self.enter(PetState::Falling);
                    self.state_left = f32::INFINITY;
                }
            }
            PetState::Idle | PetState::Sleep | PetState::Landing => {
                self.advance_timer();
            }
        }
    }

    /// Падение: гравитация + сопротивление воздуха, потолок и стены экрана,
    /// затем опора снизу. Подшаги ≤0.05 с — редкий тик не должен протыкать
    /// кромку окна насквозь и завышать скорость удара.
    fn tick_falling(&mut self, world: &World, dt: f32) {
        let mut left = dt;
        while left > 0.0 && self.state == PetState::Falling {
            let step = left.min(0.05);
            left -= step;
            // Опору ищем от ног ДО подшага: кромки выше исходной позиции
            // не считаются — на окно садимся только сверху.
            let feet_before = self.pos.y;
            self.vel.y += self.cfg.gravity * step;
            // Предел скорости падения: дальше питомец не разгоняется —
            // иначе он не падает, а мгновенно исчезает вниз.
            self.vel.y = self.vel.y.min(self.cfg.terminal_speed);
            // Затухание горизонтальной скорости: брошенный питомец
            // тормозит в полёте, а не летит по прямой до стены.
            self.vel.x *= 1.0 - (self.cfg.air_drag * step).min(1.0);
            self.pos = self.pos + self.vel * step;

            // Потолок (фаза G): за верхний край экрана питомец не улетает
            // никогда — либо цепляется, либо набивает шишку.
            if self.hit_ceiling(world) {
                continue;
            }
            // Боковые стены: цепляемся, если влетели достаточно резво,
            // иначе отскакиваем и падаем дальше.
            self.hit_walls(world);
            if self.state != PetState::Falling {
                continue;
            }

            let support = support_below(world, self.pos.x, feet_before);
            if self.pos.y >= support {
                self.land_on(support);
            }
        }
    }

    /// Ходьба по полу (земля или кромка окна). У края экрана питомец либо
    /// разворачивается, либо (фаза G) лезет на стену.
    fn tick_walk(&mut self, world: &World, dt: f32) {
        self.pos.x += self.cfg.walk_speed * self.facing.sign() * dt;
        let b = self.bounds();
        let at_edge = if b.x <= world.screen.x {
            self.pos.x = world.screen.x + self.size / 2.0;
            if self.try_wall_climb(world, Surface::WallLeft) {
                return;
            }
            self.facing = Direction::Right;
            true
        } else if b.right() >= world.screen.right() {
            self.pos.x = world.screen.right() - self.size / 2.0;
            if self.try_wall_climb(world, Surface::WallRight) {
                return;
            }
            self.facing = Direction::Left;
            true
        } else {
            false
        };
        // Поход к стене закончился разворотом (например, яйцу запретили
        // лазать по дороге): возвращаем обычный таймер прогулки, иначе
        // питомец ходил бы от края до края вечно.
        if at_edge && self.wall_trip {
            self.wall_trip = false;
            self.state_left = self.state_time + self.roll(self.cfg.walk_range);
        }
        // Пол под ногами на новой позиции: перепад в пределах STEP_SNAP
        // перешагиваем («ступеньки» окон), обрыв вниз больше порога —
        // сошли с кромки, падаем.
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

    /// Движение по стене или потолку (фаза G). На концах поверхности —
    /// переход за угол: стена -> потолок -> другая стена -> пол.
    fn tick_climb(&mut self, world: &World, dt: f32) {
        if self.surface == Surface::Floor {
            // Лазания по полу не бывает — это обычная ходьба.
            self.enter(PetState::Walk);
            return;
        }
        // Лазание идёт рывками: питомец перехватывается лапами, а не
        // едет по стене с постоянной скоростью. Средняя скорость от
        // пульсации не меняется — синус за период даёт ноль.
        let phase = self.state_time * self.cfg.climb_pull_hz * core::f32::consts::TAU;
        let pulse = 1.0 + self.cfg.climb_pull_depth * phase.sin();
        let speed = self.cfg.climb_speed() * pulse.max(0.0);
        let step = self.surface.tangent() * (speed * self.facing.sign() * dt);
        self.pos = self.pos + step;
        match self.surface {
            Surface::WallLeft | Surface::WallRight => {
                let from_left = self.surface == Surface::WallLeft;
                let b = self.bounds();
                if b.y <= world.screen.y {
                    // Дополз до потолка — уходим за угол прочь от своей стены.
                    self.switch_surface(Surface::Ceiling);
                    self.facing = if from_left {
                        Direction::Right
                    } else {
                        Direction::Left
                    };
                    self.clamp_to_surface(world);
                    return;
                }
                // Опора под ногами (земля или кромка окна у самой стены).
                let support = support_below(world, b.x + b.w / 2.0, b.bottom() - 1.0);
                if b.bottom() >= support {
                    self.switch_surface(Surface::Floor);
                    self.pos.y = support;
                    self.vel = Vec2::default();
                    self.enter(PetState::Idle);
                    self.state_left = self.roll(self.cfg.idle_range);
                    self.clamp_to_surface(world);
                    return;
                }
            }
            Surface::Ceiling => {
                let b = self.bounds();
                if b.x <= world.screen.x {
                    self.switch_surface(Surface::WallLeft);
                    self.facing = Direction::Right; // на левой стене Right = вниз
                    self.clamp_to_surface(world);
                    return;
                }
                if b.right() >= world.screen.right() {
                    self.switch_surface(Surface::WallRight);
                    self.facing = Direction::Left; // на правой стене Left = вниз
                    self.clamp_to_surface(world);
                    return;
                }
            }
            Surface::Floor => unreachable!("отсеяно выше"),
        }
        self.advance_timer();
    }

    /// Удар о потолок экрана. Возвращает true, если состояние сменилось.
    fn hit_ceiling(&mut self, world: &World) -> bool {
        let b = self.bounds();
        if b.y > world.screen.y || self.vel.y >= 0.0 {
            return false;
        }
        // Прижать макушку к потолку.
        self.pos.y += world.screen.y - b.y;
        let speed = -self.vel.y;
        if !self.grounded_only && speed <= self.cfg.ceiling_grab_speed {
            // Зацепился: висит под потолком, мордой по ходу броска.
            self.switch_surface(Surface::Ceiling);
            if self.vel.x.abs() > 1.0 {
                self.facing = if self.vel.x < 0.0 {
                    Direction::Left
                } else {
                    Direction::Right
                };
            }
            self.vel = Vec2::default();
            self.enter(PetState::Idle);
            self.state_left = self.roll(self.cfg.idle_range);
            self.clamp_to_surface(world);
        } else {
            // Слишком быстро — шишка и падение обратно.
            self.vel = Vec2::new(self.vel.x * 0.5, 0.0);
            self.enter(PetState::Bonk);
            self.state_left = self.cfg.bonk_time;
        }
        true
    }

    /// Боковые стены экрана в падении: цепляние или отскок.
    fn hit_walls(&mut self, world: &World) {
        let b = self.bounds();
        let (into_left, into_right) = (
            b.x <= world.screen.x && self.vel.x < 0.0,
            b.right() >= world.screen.right() && self.vel.x > 0.0,
        );
        if !into_left && !into_right {
            self.clamp_horizontal(world);
            return;
        }
        let wall = if into_left {
            Surface::WallLeft
        } else {
            Surface::WallRight
        };
        self.clamp_horizontal(world);
        if !self.grounded_only && self.vel.x.abs() >= self.cfg.wall_grab_speed {
            self.switch_surface(wall);
            self.vel = Vec2::default();
            // Смотрим вверх: с зацепа приятнее лезть на потолок.
            self.facing = match wall {
                Surface::WallLeft => Direction::Left,
                _ => Direction::Right,
            };
            self.enter(PetState::Idle);
            self.state_left = self.roll(self.cfg.idle_range);
            self.clamp_to_surface(world);
        } else {
            // Мягкий отскок от стены — падение продолжается.
            self.vel.x = -self.vel.x * 0.35;
        }
    }

    /// Приземление на опору `support` (пол или кромка окна).
    fn land_on(&mut self, support: f32) {
        self.pos.y = support;
        let impact = self.vel.y;
        self.vel = Vec2::default();
        self.surface = Surface::Floor;
        if self.sleep_on_land {
            self.sleep_on_land = false;
            self.enter(PetState::Sleep);
            self.state_left = self.cfg.sleep_range.1;
            return;
        }
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

    /// Попытка полезть на стену у края экрана (фаза G): бросок кубика по
    /// `w_wall_climb`. Яйцо (grounded_only) не лазает.
    fn try_wall_climb(&mut self, world: &World, wall: Surface) -> bool {
        if self.grounded_only {
            return false;
        }
        // Дошёл целенаправленно — лезет обязательно; забрёл случайно —
        // как повезёт.
        if !self.wall_trip && self.rng.u32(0..100) >= self.cfg.w_wall_climb {
            return false;
        }
        self.switch_surface(wall);
        // Наверх: на левой стене это Left, на правой — Right.
        self.facing = match wall {
            Surface::WallLeft => Direction::Left,
            _ => Direction::Right,
        };
        self.enter(PetState::Climb);
        self.state_left = self.roll(self.cfg.climb_range);
        self.clamp_to_surface(world);
        true
    }

    /// Снять питомца со стены/потолка «на ноги», не трогая состояние:
    /// сценарные пробежки демона (присутствие, фаза E) ведут его по полу.
    /// Спрайт остаётся на месте — меняется только опорная точка.
    pub fn detach_to_floor(&mut self) {
        if self.surface != Surface::Floor {
            self.switch_surface(Surface::Floor);
        }
    }

    /// Отцепиться от поверхности и полететь вниз (сам отпустил, уронили
    /// командой, яйцу запретили лазать).
    fn detach(&mut self) {
        self.switch_surface(Surface::Floor);
        self.vel = Vec2::default();
        self.enter(PetState::Falling);
        self.state_left = f32::INFINITY;
    }

    /// Сменить поверхность, сохранив положение спрайта на экране:
    /// опорная точка пересчитывается из текущего прямоугольника.
    fn switch_surface(&mut self, surface: Surface) {
        let b = self.bounds();
        self.pos = match surface {
            Surface::Floor => Vec2::new(b.x + b.w / 2.0, b.bottom()),
            Surface::Ceiling => Vec2::new(b.x + b.w / 2.0, b.y),
            Surface::WallLeft => Vec2::new(b.x, b.y + b.h / 2.0),
            Surface::WallRight => Vec2::new(b.right(), b.y + b.h / 2.0),
        };
        self.surface = surface;
        self.idle_action = IdleAction::Stand;
    }

    /// Прижать опорную точку к своей поверхности и удержать спрайт в
    /// пределах экрана вдоль неё.
    fn clamp_to_surface(&mut self, world: &World) {
        let half = self.size / 2.0;
        let (x_lo, x_hi) = (world.screen.x + half, world.screen.right() - half);
        let (y_lo, y_hi) = (world.screen.y + half, world.ground_y() - half);
        match self.surface {
            Surface::Floor => self.pos.x = self.pos.x.clamp(x_lo, x_hi.max(x_lo)),
            Surface::Ceiling => {
                self.pos.x = self.pos.x.clamp(x_lo, x_hi.max(x_lo));
                self.pos.y = world.screen.y;
            }
            Surface::WallLeft => {
                self.pos.x = world.screen.x;
                self.pos.y = self.pos.y.clamp(y_lo, y_hi.max(y_lo));
            }
            Surface::WallRight => {
                self.pos.x = world.screen.right();
                self.pos.y = self.pos.y.clamp(y_lo, y_hi.max(y_lo));
            }
        }
    }

    /// Мелкие занятия в Idle (фаза G): моргнуть, посидеть, потянуться.
    /// Только на полу и только в покое — в остальных состояниях кадр
    /// определяется самим состоянием.
    fn advance_fidget(&mut self, dt: f32) {
        if self.state != PetState::Idle || self.surface != Surface::Floor {
            self.idle_action = IdleAction::Stand;
            self.action_time = 0.0;
            self.action_left = self.roll(self.cfg.fidget_range);
            return;
        }
        self.action_time += dt;
        self.action_left -= dt;
        if self.action_left > 0.0 {
            return;
        }
        self.action_time = 0.0;
        if self.idle_action == IdleAction::Stand {
            let (action, dur) = next_idle_action(&mut self.rng);
            self.idle_action = action;
            self.action_left = dur;
        } else {
            self.idle_action = IdleAction::Stand;
            self.action_left = self.roll(self.cfg.fidget_range);
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
        if matches!(
            self.state,
            PetState::Falling | PetState::Dragged | PetState::Bonk
        ) {
            return;
        }
        if self.surface != Surface::Floor {
            // Стены и потолок принадлежат экрану, а не окнам: их геометрия
            // от снапшота не зависит — достаточно удержать питомца в
            // пределах выхода (экран мог смениться/пересчитаться).
            self.clamp_to_surface(world);
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
                    // Порог пройден — это захват. Со стены/потолка питомца
                    // при этом снимаем: в руках он всегда «ногами вниз».
                    self.switch_surface(Surface::Floor);
                    self.sleep_on_land = false;
                    // Смещение от текущей позиции питомца, чтобы он не
                    // прыгал под курсор.
                    self.drag_offset = Vec2::new(self.pos.x - p.x, self.pos.y - p.y);
                    self.enter(PetState::Dragged);
                }
                self.drag_history.rotate_left(1);
                self.drag_history[3] = (now, p);
                self.pos = p + self.drag_offset;
                // Не даём утащить за экран: спрайт целиком остаётся видимым,
                // в том числе макушка (за верхний край не уносится).
                self.clamp_horizontal(world);
                self.pos.y = self
                    .pos
                    .y
                    .clamp(world.screen.y + self.size, world.ground_y());
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
                let mut v = Vec2::new((p1.x - p0.x) / span, (p1.y - p0.y) / span);
                // Резкий флик мышью давал по 3000 px/s — питомец улетал
                // за кадр. Ограничиваем модуль, направление сохраняем.
                let speed = v.x.hypot(v.y);
                let limit = self.cfg.throw_speed_limit;
                if speed > limit {
                    v = v * (limit / speed);
                }
                self.vel = v;
                self.enter(PetState::Falling);
                true
            }
        }
    }

    fn advance_timer(&mut self) {
        if self.state_time < self.state_left {
            return;
        }
        if self.surface != Surface::Floor {
            // На стене и потолке свой набор решений (фаза G): ползти
            // дальше, повисеть или отцепиться. Спать там нельзя.
            let (next, dur) = next_state_off_floor(&self.cfg, &mut self.rng);
            if next == PetState::Falling {
                self.detach();
                return;
            }
            if next == PetState::Climb && self.rng.u32(0..100) < 25 {
                self.facing = self.facing.flip();
            }
            self.enter(next);
            self.state_left = dur;
            return;
        }
        let (mut next, mut dur) = next_state_after(self.state, &self.cfg, &mut self.rng);
        if self.grounded_only && next == PetState::Walk {
            // Яйцо не ходит (B6): решение «гулять» заменяется на Idle.
            next = PetState::Idle;
            dur = self.roll(self.cfg.idle_range);
        }
        if next == PetState::Walk {
            if self.rng.bool() {
                self.facing = self.facing.flip();
            }
            // Поход к стене: идём до края экрана, не сворачивая по таймеру.
            if !self.grounded_only && self.rng.u32(0..100) < self.cfg.w_wall_trip {
                self.wall_trip = true;
                dur = f32::INFINITY;
            }
        }
        self.enter(next);
        self.state_left = dur;
    }

    /// Случайная длительность из диапазона (lo, hi).
    fn roll(&mut self, (lo, hi): (f32, f32)) -> f32 {
        lo + (hi - lo) * self.rng.f32()
    }

    fn enter(&mut self, next: PetState) {
        if self.state != next {
            if self.state == PetState::Walk {
                // Поход к стене живёт ровно одну прогулку.
                self.wall_trip = false;
            }
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
        // Проверяем именно разворот: лазание по стенам выключено
        // (оно живёт в своих тестах фазы G).
        p.cfg.w_wall_climb = 0;
        p.cfg.w_wall_trip = 0;
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
        // Тест про физику броска: цепляние за стену отключено, иначе
        // питомец залипнет на кромке экрана вместо земли.
        p.cfg.wall_grab_speed = f32::INFINITY;
        p.cfg.w_wall_climb = 0;
        p.cfg.w_wall_trip = 0;
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
        for _ in 0..2 {
            coarse.tick(&w, 1.5);
        }
        for _ in 0..60 {
            fine.tick(&w, 0.05);
        }
        // За 3 с падения оба обязаны долететь до земли без пролёта.
        assert_eq!(coarse.pos.y, w.ground_y());
        assert_eq!(fine.pos.y, w.ground_y());
        // Состояния после приземления сравнивать нельзя: у крупного шага
        // приземление случается позже, и дальше машина поведения у них
        // расходится сама по себе. Важно, что оба уже НЕ падают.
        assert_ne!(coarse.state, PetState::Falling);
        assert_ne!(fine.state, PetState::Falling);
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

    // ---- Фаза G: стены, потолок, воздух ---------------------------------

    /// Опорная точка на каждой поверхности — точка касания: смена
    /// поверхности не двигает спрайт по экрану.
    #[test]
    fn switching_surface_keeps_sprite_in_place() {
        let mut p = pet();
        p.pos = Vec2::new(500.0, 400.0);
        let before = p.bounds();
        for surface in [Surface::Ceiling, Surface::WallLeft, Surface::WallRight] {
            p.switch_surface(surface);
            let now = p.bounds();
            assert_eq!(
                (now.x, now.y, now.w, now.h),
                (before.x, before.y, before.w, before.h),
                "{surface:?}: спрайт не должен прыгать"
            );
        }
    }

    /// Брошенный вверх питомец НИКОГДА не улетает за верхний край экрана:
    /// на умеренной скорости — цепляется за потолок.
    #[test]
    fn upward_throw_grabs_the_ceiling_instead_of_leaving_screen() {
        let w = world();
        let mut p = pet();
        // Скорость подобрана так, чтобы долететь до потолка (подъём
        // v^2/2g) и удариться мягче ceiling_grab_speed.
        p.pos = Vec2::new(900.0, 900.0);
        p.vel = Vec2::new(30.0, -1500.0);
        p.state = PetState::Falling;
        let mut min_top = f32::MAX;
        for _ in 0..300 {
            p.tick(&w, 1.0 / 60.0);
            min_top = min_top.min(p.bounds().y);
        }
        assert!(min_top >= w.screen.y, "макушка ушла за экран: {min_top}");
        assert_eq!(p.surface, Surface::Ceiling, "зацепился за потолок");
        assert_eq!(p.bounds().y, w.screen.y, "висит вплотную к потолку");
    }

    /// Очень сильный бросок вверх — шишка (Bonk) и падение обратно,
    /// но и тогда за экран питомец не уходит.
    #[test]
    fn very_hard_upward_throw_bonks_and_falls_back() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(900.0, 1000.0);
        p.vel = Vec2::new(0.0, -2600.0);
        p.state = PetState::Falling;
        let mut bonked = false;
        let mut min_top = f32::MAX;
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
            bonked |= p.state == PetState::Bonk;
            min_top = min_top.min(p.bounds().y);
        }
        assert!(bonked, "быстрый удар о потолок — это Bonk");
        assert!(
            min_top >= w.screen.y,
            "и всё равно не за экраном: {min_top}"
        );
        assert_eq!(p.pos.y, w.ground_y(), "вернулся на землю");
        assert_eq!(p.surface, Surface::Floor);
    }

    /// Бросок вбок в стену: питомец цепляется за неё, а не отскакивает.
    #[test]
    fn sideways_throw_grabs_the_wall() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(300.0, 400.0);
        p.vel = Vec2::new(-600.0, -50.0);
        p.state = PetState::Falling;
        for _ in 0..120 {
            p.tick(&w, 1.0 / 60.0);
            if p.surface == Surface::WallLeft {
                break;
            }
        }
        assert_eq!(p.surface, Surface::WallLeft, "зацепился за левую стену");
        assert_eq!(p.pos.x, w.screen.x);
        assert_eq!(p.state, PetState::Idle);
        // Прямоугольник спрайта целиком на экране.
        let b = p.bounds();
        assert!(b.x >= w.screen.x && b.right() <= w.screen.right());
    }

    /// Полный обход экрана: со стены на потолок, с потолка на другую стену,
    /// оттуда — на пол. Питомец всё время внутри экрана.
    #[test]
    fn climb_walks_around_the_screen_box() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(0.0, 800.0);
        p.surface = Surface::WallLeft;
        p.facing = Direction::Left; // вверх по левой стене
        p.state = PetState::Climb;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        let mut seen = Vec::new();
        for _ in 0..20_000 {
            p.tick(&w, 1.0 / 60.0);
            if seen.last() != Some(&p.surface) {
                seen.push(p.surface);
            }
            let b = p.bounds();
            assert!(
                b.x >= w.screen.x - 0.01
                    && b.right() <= w.screen.right() + 0.01
                    && b.y >= w.screen.y - 0.01,
                "вышел за экран: {b:?}"
            );
            // Держим питомца в режиме лазания, чтобы обойти весь периметр.
            if p.state != PetState::Climb && p.surface != Surface::Floor {
                p.state = PetState::Climb;
                p.state_left = f32::INFINITY;
                p.state_time = 0.0;
            }
            if p.surface == Surface::Floor && seen.len() > 1 {
                break;
            }
        }
        assert!(
            seen.contains(&Surface::Ceiling),
            "перешёл на потолок: {seen:?}"
        );
        assert!(
            seen.contains(&Surface::WallRight),
            "и на правую стену: {seen:?}"
        );
        assert_eq!(p.surface, Surface::Floor, "спустился на пол: {seen:?}");
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// «Поход к стене» (фаза G): рано или поздно питомец сам доходит до
    /// края экрана и лезет наверх — без этого лазание было бы видно
    /// только по случайности.
    #[test]
    fn pet_eventually_goes_climbing_on_its_own() {
        let w = world();
        let mut p = pet();
        let mut climbed = false;
        let mut max_height = f32::MAX;
        for _ in 0..200_000 {
            p.tick(&w, 1.0 / 60.0);
            if p.state == PetState::Climb || p.surface != Surface::Floor {
                climbed = true;
                max_height = max_height.min(p.bounds().y);
            }
        }
        assert!(climbed, "за час жизни питомец обязан слазить на стену");
        assert!(
            max_height < w.ground_y() - 200.0,
            "и забраться заметно выше пола: {max_height}"
        );
    }

    /// Лазание идёт рывками (перехват лапами), но средняя скорость за
    /// секунду совпадает с расчётной: пульсация не ускоряет и не тормозит.
    #[test]
    fn climb_pulses_but_keeps_average_speed() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(0.0, 900.0);
        p.surface = Surface::WallLeft;
        p.facing = Direction::Left; // вверх
        p.state = PetState::Climb;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        let start = p.pos.y;
        let (mut min_step, mut max_step) = (f32::MAX, 0.0f32);
        let mut prev = p.pos.y;
        for _ in 0..120 {
            p.tick(&w, 1.0 / 60.0);
            let step = (prev - p.pos.y).abs();
            min_step = min_step.min(step);
            max_step = max_step.max(step);
            prev = p.pos.y;
        }
        let avg = (start - p.pos.y) / 2.0; // px/s за две секунды
        let want = p.cfg.climb_speed();
        assert!(
            (avg - want).abs() < want * 0.15,
            "средняя скорость {avg} против расчётной {want}"
        );
        assert!(
            max_step > min_step * 1.5,
            "рывков нет: шаги {min_step}..{max_step}"
        );
    }

    /// Ориентация кадра: на полу — обычная, на потолке — вверх ногами
    /// («на лапках»), на стенах — профиль лицом к своей стене.
    #[test]
    fn orient_follows_surface() {
        let mut p = pet();
        p.facing = Direction::Right;
        assert_eq!(p.orient(), Orient::IDENTITY);
        p.facing = Direction::Left;
        assert!(p.orient().flip_x);
        p.surface = Surface::Ceiling;
        assert!(p.orient().flip_y, "под потолком ногами вверх");
        // На стенах питомец нарисован в профиль и смотрит НА свою стену;
        // направление движения (вверх/вниз) на зеркало не влияет, иначе он
        // перекидывался бы лицом от стены при каждом развороте.
        for facing in [Direction::Left, Direction::Right] {
            p.facing = facing;
            p.surface = Surface::WallLeft;
            assert_eq!(p.orient(), Orient::mirrored(true), "лицом к левой стене");
            p.surface = Surface::WallRight;
            assert_eq!(p.orient(), Orient::IDENTITY, "лицом к правой стене");
        }
    }

    /// «Уложить спать» на стене: питомец отцепляется и засыпает,
    /// как только приземлится.
    #[test]
    fn force_sleep_on_wall_drops_then_sleeps() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(0.0, 300.0);
        p.surface = Surface::WallLeft;
        p.state = PetState::Idle;
        p.state_left = 1000.0;
        assert!(p.force_sleep(), "команда принята");
        assert_eq!(p.state, PetState::Falling);
        assert_eq!(p.surface, Surface::Floor);
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        assert_eq!(p.state, PetState::Sleep, "уснул после приземления");
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// Захват мышью снимает питомца со стены: в руках он всегда ногами вниз.
    #[test]
    fn dragging_detaches_from_wall() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(0.0, 300.0);
        p.surface = Surface::WallLeft;
        p.state = PetState::Idle;
        let grab = Vec2::new(p.bounds().x + 5.0, p.bounds().y + 5.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(600.0, 500.0)), 0.1));
        assert_eq!(p.state, PetState::Dragged);
        assert_eq!(p.surface, Surface::Floor);
    }

    /// Яйцо (grounded_only) не лазает: ни по стенам, ни по потолку —
    /// брошенное вверх, оно набивает шишку и падает.
    #[test]
    fn egg_never_climbs() {
        let w = world();
        let mut p = pet();
        p.set_grounded_only(true);
        p.pos = Vec2::new(900.0, 900.0);
        p.vel = Vec2::new(-400.0, -600.0);
        p.state = PetState::Falling;
        for _ in 0..1200 {
            p.tick(&w, 1.0 / 60.0);
            assert_eq!(p.surface, Surface::Floor, "яйцо не цепляется");
            assert_ne!(p.state, PetState::Climb);
            assert!(p.bounds().y >= w.screen.y, "и не улетает за экран");
        }
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// Питомца, стоящего на стене, снапшот worldsense не роняет: стены
    /// принадлежат экрану, а не окнам.
    #[test]
    fn world_changes_do_not_drop_a_wall_climber() {
        let mut w = world_with(&[(0.0, 500.0, 1920.0)]);
        let mut p = pet();
        p.pos = Vec2::new(0.0, 300.0);
        p.surface = Surface::WallLeft;
        p.state = PetState::Idle;
        p.state_left = 1000.0;
        w.platforms.clear();
        p.world_changed(&w);
        assert_eq!(p.state, PetState::Idle);
        assert_eq!(p.surface, Surface::WallLeft);
    }

    /// Сопротивление воздуха гасит бросок: при равном старте питомец с
    /// драгом улетает ближе, чем при чистой баллистике.
    #[test]
    fn air_drag_shortens_a_throw() {
        let w = world();
        let mut dragged = pet();
        let mut ballistic = pet();
        ballistic.cfg.air_drag = 0.0;
        for p in [&mut dragged, &mut ballistic] {
            p.pos = Vec2::new(400.0, 200.0);
            p.vel = Vec2::new(300.0, 0.0);
            p.state = PetState::Falling;
        }
        for _ in 0..40 {
            dragged.tick(&w, 1.0 / 60.0);
            ballistic.tick(&w, 1.0 / 60.0);
        }
        assert!(
            dragged.pos.x < ballistic.pos.x - 5.0,
            "драг обязан тормозить: {} vs {}",
            dragged.pos.x,
            ballistic.pos.x
        );
    }

    /// Падение не разгоняется бесконечно: есть предел скорости, иначе
    /// питомец «телепортируется» вниз (жалоба «ебнутое ускорение»).
    #[test]
    fn falling_never_exceeds_terminal_speed() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(900.0, 100.0);
        p.state = PetState::Falling;
        let limit = p.cfg.terminal_speed;
        let mut peak = 0.0f32;
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
            peak = peak.max(p.vel.y);
        }
        assert!(peak <= limit + 1.0, "разогнался до {peak}, предел {limit}");
        // И всё-таки долетел до земли — предел не превращается в зависание.
        assert_eq!(p.pos.y, w.ground_y());
    }

    /// Резкий флик мышью не выстреливает питомцем через весь экран:
    /// модуль скорости броска ограничен, направление сохраняется.
    #[test]
    fn throw_speed_is_capped() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let grab = Vec2::new(p.pos.x, p.pos.y - 10.0);
        assert!(p.pointer(&w, PointerEvent::Press(grab), 0.0));
        // Рывок вправо-вверх: 300 px за 10 мс = 30 000 px/s «в руках».
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(1200.0, 800.0)), 0.01));
        assert!(p.pointer(&w, PointerEvent::Motion(Vec2::new(1500.0, 400.0)), 0.02));
        assert!(p.pointer(&w, PointerEvent::Release(Vec2::new(1500.0, 400.0)), 0.02));
        let speed = p.vel.x.hypot(p.vel.y);
        assert!(
            speed <= p.cfg.throw_speed_limit + 1.0,
            "бросок {speed} px/s не ограничен"
        );
        assert!(
            p.vel.x > 0.0 && p.vel.y < 0.0,
            "направление броска сохранено"
        );
    }

    /// Мелкие занятия в покое (фаза G): за долгий idle питомец успевает
    /// и моргнуть, и посидеть, и потянуться — но только на полу.
    #[test]
    fn idle_actions_cycle_on_the_floor_only() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..60_000 {
            p.tick(&w, 1.0 / 60.0);
            if p.state == PetState::Idle {
                seen.insert(format!("{:?}", p.idle_action()));
            }
        }
        assert!(seen.len() >= 3, "покой должен быть живым: {seen:?}");

        // На стене мелких занятий нет — там своя поза (cling).
        let mut p = pet();
        p.pos = Vec2::new(0.0, 300.0);
        p.surface = Surface::WallLeft;
        p.state = PetState::Idle;
        p.state_left = 1000.0;
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
            assert_eq!(p.idle_action(), IdleAction::Stand);
        }
    }

    /// Яйцу запретили лазать посреди похода к стене — вечной ходьбы от
    /// края до края не случается: таймер прогулки восстанавливается.
    #[test]
    fn cancelled_wall_trip_does_not_walk_forever() {
        let w = world();
        let mut p = pet();
        p.pos = Vec2::new(200.0, w.ground_y());
        p.state = PetState::Walk;
        p.state_time = 0.0;
        p.state_left = f32::INFINITY;
        p.facing = Direction::Left;
        p.wall_trip = true;
        p.set_grounded_only(true);
        let mut walked = 0;
        for _ in 0..3_000 {
            p.tick(&w, 1.0 / 60.0);
            if p.state == PetState::Walk {
                walked += 1;
            }
        }
        assert!(p.state_left.is_finite(), "таймер прогулки вернулся");
        assert!(walked < 3_000, "ходьба закончилась");
    }

    /// Питомец не бегает без остановки: за длинный прогон доля времени
    /// в движении заметно меньше половины (жалоба «слишком часто бегает»).
    #[test]
    fn walking_is_a_minority_of_the_time() {
        let w = world();
        let mut p = pet();
        for _ in 0..600 {
            p.tick(&w, 1.0 / 60.0);
        }
        let (mut moving, mut total) = (0u32, 0u32);
        for _ in 0..120_000 {
            p.tick(&w, 1.0 / 60.0);
            total += 1;
            if matches!(p.state, PetState::Walk | PetState::Climb) {
                moving += 1;
            }
        }
        let share = moving as f32 / total as f32;
        assert!(share < 0.45, "питомец слишком непоседлив: {share:.2}");
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

#[cfg(test)]
mod g_stats {
    use super::*;

    /// Замер баланса, а не ассерт-тест: прогон часа жизни печатает, на что
    /// уходит время питомца. Ею подбирались веса фазы G — «слишком часто
    /// бегает» проверяется числом, а не на глаз.
    ///
    /// `cargo test -p driftling-core g_stats -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_time_budget() {
        let w = World::new(Rect::new(0.0, 0.0, 1920.0, 1080.0));
        let mut p = Pet::new(Vec2::new(960.0, 100.0), 64.0, BehaviorConfig::default(), 7);
        let (mut walk, mut idle, mut sleep, mut off_floor, mut trips) = (0, 0, 0, 0, 0);
        let mut was_floor = true;
        let total = 60 * 60 * 30; // час при 30 Гц
        for _ in 0..total {
            p.tick(&w, 1.0 / 30.0);
            match p.state {
                PetState::Walk => walk += 1,
                PetState::Idle | PetState::Landing => idle += 1,
                PetState::Sleep => sleep += 1,
                _ => {}
            }
            if p.surface != Surface::Floor {
                off_floor += 1;
                if was_floor {
                    trips += 1;
                }
                was_floor = false;
            } else {
                was_floor = true;
            }
        }
        let pct = |n: i32| 100.0 * n as f32 / total as f32;
        println!(
            "час жизни: ходьба {:.0}%, покой {:.0}%, сон {:.0}%, вне пола {:.0}%, вылазок на стены {trips}",
            pct(walk),
            pct(idle),
            pct(sleep),
            pct(off_floor)
        );
    }
}
