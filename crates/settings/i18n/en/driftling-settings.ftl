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
