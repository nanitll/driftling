# driftling-ipc: control-socket errors that reach users (ctl / settings).
# en is the fallback locale and MUST stay complete.

ipc-no-runtime-dir = XDG_RUNTIME_DIR is not set — a user session (systemd/elogind) is required
ipc-connect-failed = cannot connect to socket { $path } — is the daemon running?
ipc-response-no-envelope = the daemon replied without a protocol envelope (daemon older than v{ $version }?) — restart the daemon
ipc-incompatible-protocol = incompatible protocol: client is v{ $client }, daemon is v{ $daemon } — restart the daemon
ipc-bad-response = failed to parse the daemon's response
ipc-lock-open-failed = failed to open lock file { $path }
ipc-already-running = the daemon is already running (lock { $path } is busy)
ipc-bind-failed = failed to bind socket { $path }
ipc-bad-request = bad request: { $error }
ipc-request-no-envelope = request without a protocol envelope (client older than v{ $version }?), daemon is v{ $version } — restart the daemon
