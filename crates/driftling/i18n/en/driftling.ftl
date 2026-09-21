# driftling: CLI (ctl/doctor) output and daemon IPC error responses.
# en is the fallback locale and MUST stay complete.

# --- CLI help ---
cli-about = Desktop tamagotchi that drifts between your devices
cli-about-ctl = Control the running daemon
cli-about-settings = Open the settings window (launches driftling-settings)
cli-about-summon = Summon the pet to the screen
cli-about-dismiss = Remove the pet from the screen (the daemon keeps running)
cli-about-status = Show status
cli-about-info = Pet card: name, stage, stats, characteristics
cli-about-feed = Feed the pet (regular food)
cli-about-feed-treat = Give a treat instead (mood up; overuse hurts health)
cli-about-play = Play with the pet
cli-about-mob = Send in an uninvited guest (war mode)
cli-about-mob-kind = Which one: dustball, roach, bug
cli-about-hop = Send the pet to the neighbouring monitor
cli-about-hop-dir = Where: left or right
cli-about-config = Show settings as the daemon sees them
cli-about-world = What is in the pet world right now
cli-about-toy = Get the ball out (--off to put it away)
cli-about-toy-off = Put the ball away
daemon-unknown-mob = Unknown guest "{ $value }". Available: { $known }
cli-about-sleep = Put the pet to sleep
cli-about-rename = Rename the pet
cli-about-recolor = Recolor the pet (hex color #rrggbb, e.g. e8944a)
cli-about-reload = Re-read the config and apply it on the fly
cli-about-sync = Multi-device sync (phase E)
cli-about-sync-status = Sync status: mode, last push/pull, journal cursors, presence lease
cli-about-quit = Stop the daemon
cli-about-doctor = Diagnostics: environment, socket, files, autostart (works without the daemon)

# --- ctl output ---
ctl-ok = ok
ctl-reloaded = Applied: { $applied }
ctl-nothing = nothing changed
ctl-needs-restart = Needs a daemon restart: { $items }
ctl-warning = Warning: { $text }
ctl-config-path = Settings file: { $path }
ctl-config-token-set = Sync token is set (not shown)
ctl-world-screen = Screen: { $area }, ground { $ground }
ctl-world-no-screen = Screen is not known yet
ctl-world-hidden = Pet is hidden: fullscreen window
ctl-world-empty = The world is empty so far
ctl-world-prop = { $kind }: { $state } at ({ $x }, { $y })
ctl-status = pets: { $pets }, state: { $state }, uptime: { $uptime }s
ctl-petinfo = { $name } ({ $stage }): { $state }, uptime { $uptime }s
ctl-petinfo-stats = satiety { $satiety } · energy { $energy } · mood { $mood } · health { $health }
ctl-petinfo-color = color: { $color }
ctl-petinfo-attributes = attributes: { $attributes }
ctl-recolor-bad-hex = "{ $value }" is not a color — expected hex #rrggbb, e.g. e8944a
state-dismissed = removed from screen

# --- growth stages (machine name comes from the daemon) ---
stage-egg = egg
stage-baby = baby
stage-child = child
stage-teen = teen
stage-adult = adult
settings-launch-failed = failed to launch { $program }: { $error }
settings-exited-with-error = driftling-settings exited with an error

# --- doctor ---
doctor-title = driftling doctor — environment diagnostics
doctor-wayland-ok = WAYLAND_DISPLAY = { $value }
doctor-wayland-missing = WAYLAND_DISPLAY is not set — the overlay cannot start outside a Wayland session
doctor-runtime-dir = XDG_RUNTIME_DIR = { $path }
doctor-daemon-answering = daemon is answering: protocol v{ $version } matches, pets { $pets }, state { $state }, uptime { $uptime }s
doctor-daemon-unexpected-reply = daemon is answering (unexpected reply to Status: { $reply })
doctor-daemon-not-answering = socket { $path } exists, but the daemon is not answering: { $error }
doctor-socket-missing = no socket { $path } — the daemon is not running
doctor-socket-uncheckable = { $error } — cannot check the socket
doctor-pet-ok = pet.json: pet “{ $name }” ({ $path })
doctor-pet-missing = pet.json not created yet ({ $path }) — it will appear after the daemon's first start
doctor-pet-unreadable = pet.json is unreadable ({ $path }): { $error }
doctor-journal = journal: { $events } events, { $warnings } corrupt lines skipped ({ $path })
doctor-journal-unreadable = the event journal is unreadable: { $error }
doctor-config-ok = config.toml parses ({ $path })
doctor-config-broken = config.toml is broken ({ $path }): { $error }
doctor-config-missing = config.toml is absent ({ $path }) — default settings apply
doctor-autostart-systemd = autostart: systemd unit driftling.service is enabled
doctor-autostart-desktop = autostart: found { $path }
doctor-autostart-none = autostart is not configured (neither a systemd unit nor autostart/driftling.desktop)
doctor-verdict-ok = Verdict: all good — the daemon is running and answering.
doctor-verdict-should-run = Verdict: the daemon should be running, but is not answering. Start `driftling` (or `systemctl --user restart driftling`).
doctor-verdict-not-running = Verdict: the daemon is not running, autostart is not configured. Start `driftling`.

# --- sync (ctl sync status, phase E) ---
sync-mode-off = off
sync-mode-server = own server
sync-mode-folder = synced folder
ctl-sync-mode = sync: { $mode }
ctl-sync-mode-target = sync: { $mode } — { $target }
ctl-sync-ago = { $secs } s ago
ctl-sync-never = never
ctl-sync-transfers = push: { $push } · pull: { $pull }
ctl-sync-journal =
    journal: { $events ->
        [one] { $events } event
       *[other] { $events } events
    } from { $devices ->
        [one] { $devices } device
       *[other] { $devices } devices
    }
ctl-sync-lease-ours = presence: the pet is on this device (lease is ours)
ctl-sync-lease-holder = presence: the pet is on "{ $holder }"
ctl-sync-lease-unknown = presence: lease holder not known yet
ctl-sync-error = last sync error: { $error }

# --- doctor: sync section ---
doctor-sync-off = sync is off ([sync] mode = "off")
doctor-sync-server-ok = sync server is reachable ({ $address })
doctor-sync-server-unreachable = sync server is unreachable ({ $address }): { $error }
doctor-sync-server-odd = sync server answered unexpectedly (HTTP { $status })
doctor-sync-token-ok = sync token is accepted
doctor-sync-token-bad = sync token is rejected (401) — check sync.token in config.toml
doctor-sync-folder-missing = sync folder does not exist: { $path }
doctor-sync-folder-ok =
    sync folder: { $path } ({ $files ->
        [one] { $files } journal file
       *[other] { $files } journal files
    })
doctor-sync-folder-readonly = the sync folder is not writable — the pet cannot journal there

# --- daemon IPC error responses (shown by ctl and the settings window) ---
daemon-shutting-down = the daemon is shutting down
daemon-reply-timeout = the daemon did not reply in time
daemon-output-not-ready = the output is not ready yet (no geometry)
daemon-journal-append-failed = the care event was not written to the journal: { $error }
daemon-rename-empty = the name must not be empty
daemon-pet-busy = The pet is busy right now
daemon-config-unreadable = config.toml could not be read: { $error }
daemon-config-unwritable = Could not write settings: { $error }
daemon-unknown-prop = Unknown thing "{ $value }"
daemon-prop-not-found = There is no "{ $value }" in the world right now
daemon-unknown-direction = Unknown direction "{ $value }": left or right
daemon-no-neighbour = There is no monitor on that side

# --- tray (B7) ---
tray-tooltip = Driftling
tray-summon = Summon
tray-dismiss = Remove from screen
tray-settings = Settings
tray-quit = Stop the daemon

# --- pet context menu (B3, right-click on the pet) ---
menu-feed = Feed
menu-treat = Treat
menu-play = Play
menu-toy = Ball
menu-sleep = Put to sleep
menu-settings = Settings
menu-dismiss = Remove from screen
menu-guests = Guests
menu-quiet = Quiet mode
menu-hide = Hide for fullscreen

# --- speech bubbles (B5/B6) ---
bubble-hello = Hi!
bubble-heart = ♥
bubble-annoyed = !
bubble-hiccup = Hic!
bubble-sneeze = Achoo!
bubble-bye = Bye!
bubble-birthday = Happy birthday!
bubble-wheee = Wheee!
bubble-fetch = Fetch!
bubble-scared-off = Shoo!
bubble-yuck = Blergh…

# --- pet ---
# The default name is written into the journal's Genesis event exactly
# once, at the pet's birth (it is data and survives locale changes).
default-pet-name = Driftling
