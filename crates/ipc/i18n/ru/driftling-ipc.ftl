# driftling-ipc: ошибки управляющего сокета, которые видит пользователь.

ipc-no-runtime-dir = XDG_RUNTIME_DIR не задан — нужна пользовательская сессия (systemd/elogind)
ipc-connect-failed = нет соединения с сокетом { $path } — демон не запущен?
ipc-response-no-envelope = демон ответил без конверта протокола (демон старее v{ $version }?) — перезапустите демон
ipc-incompatible-protocol = несовместимый протокол: у клиента v{ $client }, у демона v{ $daemon } — перезапустите демон
ipc-bad-response = не удалось разобрать ответ демона
ipc-lock-open-failed = не удалось открыть лок-файл { $path }
ipc-already-running = демон уже запущен (лок { $path } занят)
ipc-bind-failed = не удалось занять сокет { $path }
ipc-bad-request = некорректный запрос: { $error }
ipc-request-no-envelope = запрос без конверта протокола (клиент старее v{ $version }?), у демона v{ $version } — перезапустите демон
