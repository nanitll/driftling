// driftling-sense — сенсор «рельефа» рабочего стола для driftling (фаза D).
//
// ЧИТАЕТ геометрию и порядок окон KWin и шлёт JSON-снапшоты демону по D-Bus
// (org.driftling.WorldSense /sense Update). Строго read-only: ничего не
// двигает, не активирует и не меняет. Загружается динамически через
// org.kde.kwin.Scripting.loadDeclarativeScript, выгружается по имени плагина.
//
// Почему QML, а не plain-JS: у обычных JS-скриптов KWin 6 нет таймеров
// (setTimeout отсутствует — проверено на 6.3.6), а без таймера нельзя ни
// коалесцировать штормы событий, ни слать heartbeat. В QML есть Timer и
// DBusCall.
//
// Особенности API Plasma 6 (проверены живьём на KWin 6.3.6):
//  - в QML-скриптах нет контекстного свойства workspace — есть синглтон
//    Workspace;
//  - Workspace.windowList() из QML недоступен — берём stackingOrder
//    (bottom-to-top, снизу вверх);
//  - сигнала stackingOrderChanged нет — перестановки ловим косвенно:
//    windowActivated (KDE поднимает окно при активации) + windowAdded/
//    windowRemoved + периодический heartbeat;
//  - enum KWin.PlacementArea из QML недоступен — числовое значение 0.
import QtQuick
import org.kde.kwin 3.0

Item {
    id: root

    // Пауза после первого события перед отправкой: шторм событий (перетаскивание
    // окна шлёт frameGeometryChanged каждый кадр) сворачивается в один снапшот.
    readonly property int coalesceMs: 50
    // Периодический полный снапшот: heartbeat для детекта смерти скрипта на
    // стороне Rust и заодно самолечение пропущенных событий.
    readonly property int heartbeatMs: 5000

    // Счётчик снапшотов — только для отладки на стороне приёмника.
    property int seq: 0

    // Окна, на сигналы которых мы подписаны, — для отписки при выгрузке.
    // Соединения ПЕРЕЖИВАЮТ выгрузку скрипта: без явного disconnect каждое
    // событие окна навсегда сыпало бы в журнал TypeError об умершем root
    // (проверено живьём; wl_shimeji страдает ровно этим).
    property var hooked: []

    Timer {
        id: flushTimer
        interval: root.coalesceMs
        repeat: false
        onTriggered: root.flush()
    }

    Timer {
        id: heartbeatTimer
        interval: root.heartbeatMs
        repeat: true
        running: true
        onTriggered: root.flush()
    }

    DBusCall {
        id: push
        service: "org.driftling.WorldSense"
        path: "/sense"
        dbusInterface: "org.driftling.WorldSense"
        method: "Update"
    }

    function markDirty() {
        if (!flushTimer.running) {
            flushTimer.start();
        }
    }

    // Окно видно на текущем виртуальном рабочем столе?
    function onCurrentDesktop(w) {
        if (w.onAllDesktops) {
            return true;
        }
        var cur = Workspace.currentDesktop;
        var ds = w.desktops;
        if (!ds || !cur) {
            return true; // нет данных — не отфильтровываем
        }
        for (var i = 0; i < ds.length; ++i) {
            if (ds[i] === cur || (ds[i] && ds[i].id === cur.id)) {
                return true;
            }
        }
        return false;
    }

    // Подписка на события одного окна, меняющие «рельеф». С окном соединения
    // умирают сами, а вот выгрузку скрипта НЕ переживать они не умеют —
    // отписка в Component.onDestruction по списку hooked.
    function hookWindow(w) {
        w.frameGeometryChanged.connect(root.markDirty);
        w.minimizedChanged.connect(root.markDirty);
        w.fullScreenChanged.connect(root.markDirty);
        w.desktopsChanged.connect(root.markDirty);
        w.outputChanged.connect(root.markDirty);
        // Конец интерактивного перетаскивания/ресайза — финальная геометрия.
        w.interactiveMoveResizeFinished.connect(root.markDirty);
        root.hooked.push(w);
    }

    function unhookWindow(w) {
        w.frameGeometryChanged.disconnect(root.markDirty);
        w.minimizedChanged.disconnect(root.markDirty);
        w.fullScreenChanged.disconnect(root.markDirty);
        w.desktopsChanged.disconnect(root.markDirty);
        w.outputChanged.disconnect(root.markDirty);
        w.interactiveMoveResizeFinished.disconnect(root.markDirty);
    }

    // Именованный обработчик windowAdded: анонимную функцию нельзя было бы
    // отписать при выгрузке.
    function onWindowAdded(w) {
        hookWindow(w);
        markDirty();
    }

    function flush() {
        var wins = Workspace.stackingOrder; // bottom-to-top
        var out = [];
        var anyFs = false;
        for (var i = 0; i < wins.length; ++i) {
            var w = wins[i];
            // Здесь только фильтр по типу окна (панели/доки/попапы/удалённые);
            // смысловой фильтр (minimized, свои окна, skipTaskbar) — на стороне
            // Rust, где он покрыт юнит-тестами.
            if (!w.normalWindow || w.popupWindow || w.deleted || w.hidden) {
                continue;
            }
            if (!onCurrentDesktop(w)) {
                continue;
            }
            var g = w.frameGeometry;
            if (!g) {
                continue;
            }
            if (w.fullScreen && !w.minimized) {
                anyFs = true;
            }
            out.push({
                iid: String(w.internalId),
                x: g.x,
                y: g.y,
                w: g.width,
                h: g.height,
                minimized: w.minimized === true,
                fullscreen: w.fullScreen === true,
                skipTaskbar: w.skipTaskbar === true,
                cls: String(w.resourceClass)
            });
        }
        root.seq = (root.seq + 1) % 1000000000;
        var snap = {
            windows: out,
            anyFullscreen: anyFs,
            seq: root.seq
        };
        try {
            // 0 = PlacementArea: область размещения с учётом панелей (struts).
            var wa = Workspace.clientArea(0, Workspace.activeScreen, Workspace.currentDesktop);
            snap.workArea = {
                x: wa.x,
                y: wa.y,
                w: wa.width,
                h: wa.height
            };
        } catch (e) {
            // Нет workArea — приёмник возьмёт низ экрана.
        }
        // Рабочая область КАЖДОГО экрана вместе с его геометрией: демон
        // живёт на своём выходе и обязан взять область именно его, а не
        // активного (на двух мониторах это разные вещи — питомец иначе
        // ходит по чужим панелям и висит над чужим полом).
        try {
            var areas = [];
            var screens = Workspace.screens;
            for (var s = 0; s < screens.length; ++s) {
                var out = screens[s];
                var g = out.geometry;
                var a = Workspace.clientArea(0, out, Workspace.currentDesktop);
                areas.push({
                    sx: g.x,
                    sy: g.y,
                    sw: g.width,
                    sh: g.height,
                    x: a.x,
                    y: a.y,
                    w: a.width,
                    h: a.height
                });
            }
            if (areas.length > 0) {
                snap.workAreas = areas;
            }
        } catch (e) {
            // Старый KWin без Workspace.screens — остаётся workArea выше.
        }
        push.arguments = [JSON.stringify(snap)];
        push.call();
    }

    Component.onCompleted: {
        var wins = Workspace.stackingOrder;
        for (var i = 0; i < wins.length; ++i) {
            hookWindow(wins[i]);
        }
        Workspace.windowAdded.connect(root.onWindowAdded);
        Workspace.windowRemoved.connect(root.markDirty);
        Workspace.windowActivated.connect(root.markDirty);
        Workspace.currentDesktopChanged.connect(root.markDirty);
        Workspace.virtualScreenGeometryChanged.connect(root.markDirty);
        flush(); // стартовый снапшот — сразу, не дожидаясь событий
    }

    // Выгрузка (unloadScript, рестарт демона): снять ВСЕ соединения — и с
    // Workspace, и с каждого зацепленного окна. Умершие окна кидаются —
    // глотаем и идём дальше, чтобы одно мёртвое не оставило хвост живых.
    Component.onDestruction: {
        Workspace.windowAdded.disconnect(root.onWindowAdded);
        Workspace.windowRemoved.disconnect(root.markDirty);
        Workspace.windowActivated.disconnect(root.markDirty);
        Workspace.currentDesktopChanged.disconnect(root.markDirty);
        Workspace.virtualScreenGeometryChanged.disconnect(root.markDirty);
        for (var i = 0; i < root.hooked.length; ++i) {
            try {
                unhookWindow(root.hooked[i]);
            } catch (e) {
                // окно уже уничтожено — его соединения умерли вместе с ним
            }
        }
        root.hooked = [];
    }
}
