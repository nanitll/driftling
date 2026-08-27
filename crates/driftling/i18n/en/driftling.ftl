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
cli-about-sleep = Put the pet to sleep
cli-about-rename = Rename the pet
cli-about-reload = Re-read the config and apply it on the fly
cli-about-quit = Stop the daemon
cli-about-doctor = Diagnostics: environment, socket, files, autostart (works without the daemon)

# --- ctl output ---
ctl-ok = ok
ctl-status = pets: { $pets }, state: { $state }, uptime: { $uptime }s
ctl-petinfo = { $name } ({ $stage }): { $state }, uptime { $uptime }s
ctl-petinfo-stats = satiety { $satiety } · energy { $energy } · mood { $mood } · health { $health }
ctl-petinfo-attributes = attributes: { $attributes }
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

# --- daemon IPC error responses (shown by ctl and the settings window) ---
daemon-shutting-down = the daemon is shutting down
daemon-reply-timeout = the daemon did not reply in time
daemon-output-not-ready = the output is not ready yet (no geometry)
daemon-journal-append-failed = the care event was not written to the journal: { $error }
daemon-rename-empty = the name must not be empty
daemon-config-unreadable = config.toml could not be read: { $error }

# --- tray (B7) ---
tray-tooltip = Driftling
tray-summon = Summon
tray-dismiss = Remove from screen
tray-settings = Settings
tray-quit = Stop the daemon

# --- pet ---
# The default name is written into the journal's Genesis event exactly
# once, at the pet's birth (it is data and survives locale changes).
default-pet-name = Driftling
