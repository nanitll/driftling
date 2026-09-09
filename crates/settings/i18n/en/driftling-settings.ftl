# driftling-settings: settings window UI.
# en is the fallback locale and MUST stay complete.

# --- Pet state badge ---
state-absent = Removed from screen
state-idle = Resting
state-walk = Walking
state-sleep = Sleeping
state-falling = Falling
state-dragged = Held
state-landing = Landed
state-climb = Climbing
state-bonk = Bumped its head
state-roll = Rolling
state-unknown = Unknown

# --- Uptime (CLDR plural rules) ---
uptime-hours =
    { $hours ->
        [one] { $hours } hour
       *[other] { $hours } hours
    }
uptime-minutes =
    { $minutes ->
        [one] { $minutes } minute
       *[other] { $minutes } minutes
    }
uptime-seconds =
    { $seconds ->
        [one] { $seconds } second
       *[other] { $seconds } seconds
    }

# --- Sidebar ---
nav-pet = Pet
nav-app = Application
nav-debug = Debug
badge-checking = Checking…
daemon-running = Daemon is running
daemon-not-running = Daemon is not running

# --- Pet page ---
default-pet-name = Driftling
badge-no-data = No data
online-for = online for { $uptime }
btn-summon = Summon
btn-dismiss = Remove from screen
msg-pet-summoned = Pet summoned
msg-pet-dismissed = Pet removed
daemon-error = Daemon error: { $error }
generic-error = Error: { $error }
rename-hint = Rename
msg-renamed = Pet renamed
msg-rename-empty = The name cannot be empty
btn-feed = Feed
btn-treat = Treat
btn-play = Play
btn-sleep = Put to sleep
msg-fed = The pet has been fed
msg-treat-given = Treat given
msg-played = You played with the pet
msg-put-to-sleep = The pet was put to bed
daemon-down-hint = The daemon is not running — start it to take care of the pet.
btn-start-daemon = Start the daemon
msg-daemon-starting = Starting the daemon…

# --- Appearance card (pet color) ---
section-appearance = Appearance
appearance-hint = The pet's color also drives the app's accent color.
color-custom = Custom color
msg-recolored = Pet recolored
color-greige = Greige
color-amber = Amber
color-mint = Mint
color-sky = Sky
color-rose = Rose
color-slate = Slate
color-sand = Sand
color-violet = Violet

# --- Growth stage chip ---
stage-egg = Egg
stage-baby = Baby
stage-child = Child
stage-teen = Teen
stage-adult = Adult

# --- Condition card ---
section-condition = Condition
stat-satiety = Satiety
stat-energy = Energy
stat-mood = Mood
stat-health = Health
condition-unavailable = The daemon is not running — the pet's condition is unavailable.

section-stats = Characteristics
stat-speed = Speed
stat-speed-value = { $value } px/s
stat-curiosity = Restlessness
stat-sleepiness = Sleepiness
stat-size = Size
stat-size-value = { $value } px
stat-sleep = Sleep
stat-sleep-value = { $min }–{ $max } s
stats-unavailable = The daemon is not running — characteristics are unavailable.
stats-grow-note = Characteristics grow together with the pet

# --- Application page ---
section-launch = Startup
autostart-title = Start on login
autostart-desc = The daemon will start together with the desktop
msg-autostart-on = Autostart enabled
msg-autostart-off = Autostart disabled
section-daemon = Daemon
uptime-label = · uptime { $uptime }
btn-stop-daemon = Stop the daemon
btn-stop-confirm = Really stop?
msg-daemon-stopped = Daemon stopped
section-about = About
about-desc = A pet for the Wayland desktop. It lives on the bottom edge of the screen, walks, sleeps and falls into your hands.

# --- Sync card (phase E) ---
section-sync = Sync
sync-mode-off = Off
sync-mode-server = My server
sync-mode-folder = Folder
sync-off-hint = The pet lives only on this device.
sync-address-label = Server address
sync-address-hint = driftling-server over plain http — keep it on LAN/VPN or behind a local proxy; the token comes from `driftling-server account add`.
sync-token-label = Token
sync-folder-label = Journal folder
sync-folder-hint = A folder synced by Syncthing/Nextcloud: journals move there, one file per device. pet.json always stays local.
msg-sync-applied = Saved — the daemon has reloaded the sync settings
msg-sync-saved-daemon-down = Saved; it will apply when the daemon starts
sync-status-line = push: { $push } · pull: { $pull }
sync-status-ago = { $secs } s ago
sync-status-never = never
sync-status-journal = { $events } events · { $devices } devices
sync-status-lease-ours = the pet is here
sync-status-lease-other = the pet is on "{ $holder }"
sync-status-error = sync error: { $error }

# --- Debug page ---
debug-warning = Admin panel. Direct editing of characteristics is for debugging; in the game they will grow through caring for the pet.
section-pet-attrs = Pet characteristics
slider-size = Size, px
slider-speed = Speed, px/s
slider-curiosity = Restlessness
slider-sleepiness = Sleepiness
sleep-secs-label = Sleep, s:
sleep-from = from
sleep-to = to
debug-clamp-note = The daemon clamps values: restlessness + sleepiness ≤ 100, min sleep ≤ max.
btn-apply = Apply
msg-applied = Applied and saved
btn-reset = Reset to defaults
section-raw-stats = Raw stats
stat-stage = Stage
debug-growth-hint = Accelerated growth testing: start the daemon with DRIFTLING_GROWTH_SCALE=1440 (one real minute = one in-game day).
section-raw = Raw daemon response
raw-none = No response from the daemon.

# --- Navigation (M0.6 redesign) ---
nav-world = World
nav-devices = Devices
nav-advanced = Advanced
nav-hotkey = Ctrl+{ $n }

# --- Common messages ---
msg-settings-applied = Settings applied
msg-settings-saved-offline = Saved — will apply when the daemon starts
msg-config-rescued = The old config.toml could not be read; it is kept as { $file }
msg-nothing-changed = Nothing changed
msg-needs-restart = Not everything applied: { $items } needs a daemon restart
msg-ui-saved = Window settings saved
msg-attrs-applied = Temperament applied
msg-treated = Treat given
msg-sleeping = The pet is off to sleep
msg-guest-called = Guest summoned
msg-ride-called = Vehicle sent over
msg-ride-stopped = The pet got off
msg-ball-out = Ball is on screen
msg-ball-away = Ball put away
msg-daemon-stopping = Stopping the daemon

# --- Pet page ---
btn-dismiss-hint = The pet waves goodbye and runs off the screen edge
btn-take-current = Take current
pet-size = Size
pet-size-hint = Also sets the world scale: weight, gravity and thing sizes follow it
pet-journal-note = Everything here changes the pet itself: events go to the journal and to your other devices
section-temper = Temperament
temper-preset = Temperament
temper-hint = Ready-made sets; hand tuning is below
temper-calm = Calm
temper-normal = Normal
temper-lively = Restless
temper-custom = Custom
temper-fine = Fine tuning
attr-walk-speed = Walking speed
attr-curiosity = Curiosity
attr-sleepiness = Sleepiness
attr-sleep-range = Sleep lasts, s

# --- World page ---
section-peace = Peace
section-guests = Guests
section-rides = Rides
section-things = Things
section-mess = Mess
world-local-note = These are this computer's rules: they do not travel to your other devices
world-offline-note = The daemon is not running: settings are saved to the file and apply on next start
world-muted-by-quiet = Currently muted by quiet mode
world-quiet = Quiet mode
world-quiet-hint = The pet starts nothing on its own: no guests, no rides, no trips home
world-hide = Hide for fullscreen windows
world-hide-hint = With a house around it hides inside and comes back out to wave
world-grace = Wait before hiding
world-grace-hint = Brief fullscreen flashes are ignored
world-sick = Gets motion sick
world-sick-hint = Long shaking makes the pet queasy; off means you can shake all you like
world-surprises = Surprise frequency
world-surprises-hint = Multiplier for everything that happens on its own: 0 means only what you do
world-war = Uninvited guests
world-war-hint = A dust ball, a roach or a bug drops by now and then and gets chased off. No damage
world-mob-every = They drop by roughly every
world-mob-kinds = Who may show up
world-kinds-hint = Nothing picked means all of them
world-guest-manual-hint = You can summon one even with this off
btn-summon-guest = Summon a guest
world-rides = Arrive on their own
world-rides-hint = A vehicle is sent over now and then and the pet takes a ride
world-ride-every = Sent over roughly every
world-ride-kinds = Which vehicles
world-riding-now = Riding now: { $kind }
btn-ride-now = Ride now
btn-stop-ride = Get off
world-bowl = Keep a bowl
world-bowl-hint = The first feeding puts a bowl down and the pet eats there from then on
world-bed = Keep a bed
world-bed-hint = "Put to sleep" walks the pet to its bed instead of the bare floor
world-house = House
world-house-hint = Its own corner: the pet hides there from fullscreen windows and comes out to wave
world-house-age = Appears at age
world-ball = Ball
world-ball-hint = Not a setting but a thing: the ball is temporary, stays out of the journal and off your other devices
world-scene = In the world right now
world-scene-empty = Nothing yet
world-tag-kept = kept
world-tag-temporary = temporary
world-tag-guest = guest
world-puddles = Puddles and mopping
world-puddles-hint = Off means no new puddles; an existing one still gets mopped up
world-sneezes = Sneezes
world-sneezes-hint = A rare sneeze when idle
world-litter = Dust ball leaves litter
world-litter-hint = The guest trails dust behind it
world-shadow = Shadow under the pet
world-shadow-hint = Without a shadow the pet visually floats off the floor
world-chore = Starts cleaning after
world-chore-hint = How long the pet is left alone before it reaches for the mop

# --- Devices page ---
section-where-lives = Where the pet lives
section-sync-status = What is happening
sync-mode = Mode
sync-mode-hint = One computer, your own server, or a shared folder
sync-address = Server address
sync-address-empty = Sync needs an address
sync-address-scheme = The address must start with http:// or https://
sync-token = Token
sync-token-hint = Empty keeps the current one; the daemon never hands the token out
sync-token-kept = set, keep as is
sync-token-empty = not set
sync-folder = Folder
sync-folder-empty = Pick a folder
sync-folder-missing = No such folder
sync-folder-not-dir = That is not a folder
sync-status-unavailable = The daemon said nothing about sync
sync-not-applied = Settings are not applied yet
sync-last-push = Pushed
sync-last-pull = Pulled
sync-events = Events / devices
sync-presence = The pet is now
sync-here = here
sync-elsewhere = on "{ $device }"
sync-nobody = nobody's
sync-never = never yet
sync-seconds-ago = { $secs }s ago
sync-reload-hint = If you edited config.toml by hand
btn-discard = Discard
btn-open = Open
btn-reload = Reload

# --- App page ---
section-window = Window
section-service = Service
ui-language = Interface language
ui-language-hint = Applies after the window restarts; the pet's speech bubbles follow the system language
ui-start-page = Open on page
ui-advanced = Show "Advanced"
ui-advanced-hint = Same as starting with --debug
ui-text-scale = Pet label size
ui-text-scale-hint = Speech bubbles and menu labels; helps on high-density screens
lang-system = System
lang-ru = Русский
lang-en = English
service-config = Settings file
service-doctor = Check the installation
service-doctor-hint = Runs driftling ctl doctor and shows the report
service-doctor-running = Checking…
service-doctor-empty = The doctor said nothing
about-version = Version
btn-run = Run

# --- Advanced page ---
advanced-warning = Technical knobs. Easy to make the pet behave oddly
section-physics = World physics
physics-height = Pet height in real life
physics-height-hint = A smaller pet means a smaller world: the same fall looks faster
physics-room = Room is about { $w } × { $h } m
stat-health-hint = A hidden stat: not shown on the ordinary pages
world-screen = Work area
world-screen-unknown = screen not known yet
world-hidden-now = The pet is hidden right now: fullscreen window
raw-petinfo = PetInfo (JSON)
section-dangerous = Dangerous
dangerous-ride = Send a specific vehicle
dangerous-ride-hint = Bypasses the allowed-kinds list
dangerous-mob = Summon a specific guest
dangerous-mob-hint = Bypasses war mode and the kinds list
btn-reset-default = Reset to default

# --- Names of things in the world ---
prop-mop = Mop
prop-bowl = Bowl
prop-bed = Bed
prop-ball = Ball
prop-house = House
prop-skate = Skateboard
prop-bike = Bicycle
prop-moped = Moped
prop-car = Car
prop-copter = Helicopter
prop-plane = Plane
prop-dustball = Dust ball
prop-roach = Roach
prop-bug = Bug
