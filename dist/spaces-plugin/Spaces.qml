// omaspace Spaces panel: your machines along the top edge, each with its
// workspaces. Drag a window to the top (SUPER-drag) and drop it on a machine
// to give it there, or open it with SUPER+CTRL+SHIFT+O and use the keyboard.
//
// While a window is being dragged Hyprland holds the pointer, so this
// surface never sees the drag itself: omaspace-spaces-drag (bound to
// SUPER+mouse:272 press/release) feeds it the cursor with hover() and ends
// it with drop(). Everything it does runs omarchy-omaspace, like the menu.
//
// MIT. Part of omaspace.
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui

Item {
  id: root

  property var shell: null
  property var manifest: null

  property bool opened: false
  property string mode: "keyboard"      // "keyboard" | "drag"
  property string dragAddress: ""
  property string dragTitle: ""
  property string monitorName: ""
  property var machines: []
  property bool loading: false
  property string loadError: ""
  property int selMachine: 0
  property int selWs: 1
  property int hoverMachine: -1
  property int hoverWs: -1
  property string lastAction: ""

  readonly property string home: Quickshell.env("HOME")
  readonly property string omaspace: home + "/.local/bin/omaspace"
  readonly property string helper: home + "/.local/bin/omarchy-omaspace"

  property color background: Color.menu.background
  property color foreground: Color.menu.text
  property color border: Color.menu.border
  property var borderSpec: Border.surfaceSpec("menu", "border", border, Math.max(1, Style.space(2)))
  property color selectedBackground: Color.menu.selectedBackground
  property color selectedText: Color.menu.selectedText
  property color accent: Color.accent
  readonly property int cornerRadius: Style.cornerRadius
  property string fontFamily: Style.font.menuFamily
  property int pad: Style.spacing.panelPadding
  property int cellSize: Math.max(Style.space(34), Style.font.heading * 2 + Style.space(6))
  property int tileWidth: cellSize * 3 + Style.space(4) * 2 + pad * 2

  // ---------------------------------------------------------------- open/close

  function open(payloadJson) {
    var p = {}
    try { p = JSON.parse(payloadJson || "{}") } catch (e) { p = {} }
    root.mode = p.mode === "drag" ? "drag" : "keyboard"
    root.dragAddress = p.address || ""
    root.dragTitle = p.title || ""
    root.monitorName = p.monitor || ""
    root.hoverMachine = -1
    root.hoverWs = -1
    root.opened = true
    root.refresh()
    if (root.mode === "keyboard") Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() { root.opened = false }

  function dismiss() {
    root.opened = false
    if (root.shell && typeof root.shell.hide === "function")
      root.shell.hide((root.manifest && root.manifest.id) || "omaspace.spaces")
  }

  function refresh() {
    if (spacesProc.running) return
    root.loading = true
    spacesProc.running = true
  }

  function loaded(text) {
    root.loading = false
    try {
      var data = JSON.parse(text)
      root.machines = data.machines || []
      root.loadError = ""
    } catch (e) {
      root.loadError = "omaspace spaces failed"
    }
    if (root.selMachine >= root.machines.length) root.selMachine = 0
    // Start on the first other machine: that is where things are given.
    if (root.mode === "keyboard" && root.machines.length > 1 && root.machines[root.selMachine].here) root.selMachine = 1
    var here = root.hereMachine()
    if (here && here.workspaces) root.selWs = here.workspaces.active || 1
  }

  function hereMachine() {
    for (var i = 0; i < root.machines.length; i++) if (root.machines[i].here) return root.machines[i]
    return null
  }

  // --------------------------------------------------------------- the actions

  function run(argv) {
    root.lastAction = argv.join(" ")
    Quickshell.execDetached(argv)
  }

  function give(machineIndex, ws) {
    var m = root.machines[machineIndex]
    if (!m) return
    if (m.here) {
      // This machine: switch to that workspace.
      root.run(["hyprctl", "dispatch", "hl.dsp.focus({ workspace = \"" + ws + "\" })"])
    } else {
      root.run([root.helper, "send", m.name])
    }
    root.dismiss()
  }

  function takeBack(machineIndex, ws) {
    var m = root.machines[machineIndex]
    if (!m || m.here) return
    root.run([root.helper, "pull", m.name, String(ws)])
    root.dismiss()
  }

  function act(name) {
    if (name === "synced") { root.run([root.helper, "synced"]); root.dismiss(); return }
    var m = root.machines[root.selMachine]
    if (!m || m.here) return
    if (name === "watch") root.run([root.helper, "watch", m.name, String(root.selWs)])
    else if (name === "files") root.run([root.helper, "send-file", m.name])
    else if (name === "get") root.run([root.helper, "get-file", m.name])
    else if (name === "sync") root.run([root.helper, "sync-folder", m.name])
    root.dismiss()
  }

  // Dropped a dragged window at (x, y) on this panel's screen.
  function dropAt(x, y) {
    var hit = root.hitTest(x, y)
    var m = root.machines[hit.machine]
    if (!m || !root.dragAddress) {
      root.lastAction = "cancelled"
    } else if (m.here) {
      if (hit.ws > 0)
        root.run(["hyprctl", "dispatch", "hl.dsp.window.move({ workspace = \"" + hit.ws + "\", follow = false, window = \"address:" + root.dragAddress + "\" })"])
      else root.lastAction = "cancelled"
    } else {
      var argv = [root.helper, "give-window", m.name, root.dragAddress]
      if (hit.ws > 0) argv.push(String(hit.ws))
      root.run(argv)
    }
    root.dismiss()
    return root.lastAction
  }

  // ------------------------------------------------------------ hit testing

  function hitTest(x, y) {
    for (var i = 0; i < tiles.count; i++) {
      var tile = tiles.itemAt(i)
      if (!tile) continue
      var p = tile.mapFromItem(null, x, y)
      if (p.x < 0 || p.y < 0 || p.x > tile.width || p.y > tile.height) continue
      var ws = -1
      for (var j = 0; j < tile.cells.count; j++) {
        var cell = tile.cells.itemAt(j)
        var q = cell.mapFromItem(null, x, y)
        if (q.x >= 0 && q.y >= 0 && q.x <= cell.width && q.y <= cell.height) ws = cell.wsId
      }
      return { machine: i, ws: ws }
    }
    return { machine: -1, ws: -1 }
  }

  // ------------------------------------------------- IPC (omarchy-shell call)

  function hover(arg) {
    var xy = String(arg).split(/[ ,]/)
    var hit = root.hitTest(Number(xy[0]), Number(xy[1]))
    root.hoverMachine = hit.machine
    root.hoverWs = hit.ws
    return JSON.stringify(hit)
  }

  function drop(arg) {
    var xy = String(arg).split(/[ ,]/)
    return root.dropAt(Number(xy[0]), Number(xy[1]))
  }

  function prefetch(arg) { root.refresh(); return "ok" }

  // Where a cell is on screen, for tests: "machine ws" -> "x y" (centre).
  function cellCenter(arg) {
    var a = String(arg).split(" ")
    var tile = tiles.itemAt(Number(a[0]))
    if (!tile) return ""
    var cell = Number(a[1]) > 0 ? tile.cells.itemAt(Number(a[1]) - 1) : tile
    if (!cell) return ""
    var p = cell.mapToItem(null, cell.width / 2, cell.height / 2)
    return Math.round(p.x) + " " + Math.round(p.y)
  }

  function debugState(arg) {
    return JSON.stringify({
      opened: root.opened, mode: root.mode, loading: root.loading, error: root.loadError,
      machines: root.machines.map(function(m) { return m.name }),
      selected: { machine: root.selMachine, ws: root.selWs },
      hover: { machine: root.hoverMachine, ws: root.hoverWs },
      dragAddress: root.dragAddress, lastAction: root.lastAction,
      screen: panel.screen ? panel.screen.name : ""
    })
  }

  Process {
    id: spacesProc
    command: [root.omaspace, "spaces"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.loaded(text) }
    onExited: function(code) { if (code !== 0) { root.loading = false; root.loadError = "omaspace spaces exited " + code } }
  }

  // ------------------------------------------------------------------ the UI

  PanelWindow {
    id: panel
    visible: root.opened
    screen: {
      var all = Quickshell.screens
      for (var i = 0; i < all.length; i++) if (all[i].name === root.monitorName) return all[i]
      return all.length ? all[0] : null
    }
    anchors { top: true; left: true; right: true }
    implicitHeight: card.height + Style.gapsOut * 2
    color: "transparent"
    WlrLayershell.namespace: "omaspace-spaces"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: root.mode === "keyboard" ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None
    exclusionMode: ExclusionMode.Ignore

    MouseArea { anchors.fill: parent; onClicked: root.dismiss() }

    BorderSurface {
      id: card
      anchors.horizontalCenter: parent.horizontalCenter
      anchors.top: parent.top
      anchors.topMargin: Style.gapsOut
      width: Math.min(column.implicitWidth + root.pad * 2, panel.width - Style.gapsOut * 2)
      height: column.implicitHeight + root.pad * 2
      radius: root.cornerRadius
      color: root.background
      borderSpec: root.borderSpec
      padding: root.pad

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(event) {
          var n = root.machines.length
          if (event.key === Qt.Key_Escape) root.dismiss()
          else if (event.key === Qt.Key_Left && n) root.selMachine = (root.selMachine - 1 + n) % n
          else if (event.key === Qt.Key_Right && n) root.selMachine = (root.selMachine + 1) % n
          else if (event.key === Qt.Key_Up) root.selWs = Math.max(1, root.selWs - 3)
          else if (event.key === Qt.Key_Down) root.selWs = Math.min(9, root.selWs + 3)
          else if (event.key >= Qt.Key_1 && event.key <= Qt.Key_9) root.selWs = event.key - Qt.Key_0
          else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) root.give(root.selMachine, root.selWs)
          else if (event.key === Qt.Key_T) root.takeBack(root.selMachine, root.selWs)
          else if (event.key === Qt.Key_W) root.act("watch")
          else if (event.key === Qt.Key_F) root.act("files")
          else if (event.key === Qt.Key_G) root.act("get")
          else if (event.key === Qt.Key_S) root.act("sync")
          else if (event.key === Qt.Key_Y) root.act("synced")
          else return
          event.accepted = true
        }
      }

      Column {
        id: column
        x: card.contentLeftInset
        y: card.contentTopInset
        spacing: Style.spacing.md

        Text {
          textFormat: Text.PlainText
          text: root.mode === "drag"
            ? "Drop on a machine to give " + (root.dragTitle || "this window")
            : (root.loading && !root.machines.length ? "Looking for your machines…" : "Spaces")
          color: root.foreground
          font.family: root.fontFamily
          font.pixelSize: Style.font.title
        }

        Row {
          id: row
          spacing: Style.spacing.md

          Repeater {
            id: tiles
            model: root.machines

            delegate: Rectangle {
              id: tile
              required property var modelData
              required property int index
              property alias cells: cellRepeater
              readonly property bool selected: root.mode === "keyboard" ? index === root.selMachine : index === root.hoverMachine
              width: root.tileWidth
              height: tileColumn.implicitHeight + root.pad * 2
              radius: root.cornerRadius
              color: selected ? root.selectedBackground : "transparent"
              border.width: Math.max(1, Style.space(1))
              border.color: selected ? root.accent : Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.15)

              Column {
                id: tileColumn
                x: root.pad
                y: root.pad
                spacing: Style.space(6)

                Column {
                  Text {
                    textFormat: Text.PlainText
                    width: root.tileWidth - root.pad * 2
                    elide: Text.ElideRight
                    text: tile.modelData.name
                    color: tile.selected ? root.selectedText : root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.heading
                    font.bold: true
                  }
                  Text {
                    textFormat: Text.PlainText
                    text: tile.modelData.here ? "this machine"
                      : tile.modelData.error ? "not answering"
                      : tile.modelData.sync_errors ? "sync failing (S to see)"
                      : tile.modelData.synced ? (tile.modelData.synced === 1 ? "1 folder in sync" : tile.modelData.synced + " folders in sync")
                      : "on your tailnet"
                    color: root.foreground
                    opacity: 0.6
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.caption
                  }
                }

                Grid {
                  columns: 3
                  spacing: Style.space(4)
                  Repeater {
                    id: cellRepeater
                    model: 9
                    delegate: Rectangle {
                      id: cell
                      required property int index
                      readonly property int wsId: index + 1
                      readonly property var ws: {
                        var list = (tile.modelData.workspaces && tile.modelData.workspaces.workspaces) || []
                        for (var i = 0; i < list.length; i++) if (list[i].id === wsId) return list[i]
                        return { id: wsId, windows: [] }
                      }
                      readonly property bool active: !!tile.modelData.workspaces && tile.modelData.workspaces.active === wsId
                      readonly property bool picked: root.mode === "keyboard"
                        ? (tile.index === root.selMachine && wsId === root.selWs)
                        : (tile.index === root.hoverMachine && wsId === root.hoverWs)
                      width: root.cellSize
                      height: root.cellSize
                      radius: root.cornerRadius
                      color: picked ? root.accent : (active ? root.selectedBackground : "transparent")
                      border.width: Math.max(1, Style.space(1))
                      border.color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, cell.ws.windows.length ? 0.45 : 0.15)

                      Text {
                        textFormat: Text.PlainText
                        anchors.centerIn: parent
                        anchors.verticalCenterOffset: cell.ws.windows.length ? -Style.space(5) : 0
                        text: cell.wsId
                        color: cell.picked ? root.background : root.foreground
                        opacity: cell.ws.windows.length || cell.active ? 1 : 0.45
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.heading
                      }
                      Text {
                        textFormat: Text.PlainText
                        visible: cell.ws.windows.length > 0
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: Style.space(3)
                        text: "•".repeat(Math.min(cell.ws.windows.length, 4))
                        color: cell.picked ? root.background : root.accent
                        font.pixelSize: Style.font.caption
                      }

                      MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        onContainsMouseChanged: if (containsMouse && root.mode === "keyboard") { root.selMachine = tile.index; root.selWs = cell.wsId }
                        onClicked: root.give(tile.index, cell.wsId)
                      }
                    }
                  }
                }

                Text {
                  textFormat: Text.PlainText
                  width: root.cellSize * 3 + Style.space(8)
                  elide: Text.ElideRight
                  visible: !tile.modelData.here
                  text: tile.modelData.error ? tile.modelData.error : ""
                  color: root.foreground
                  opacity: 0.6
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }
              }
            }
          }
        }

        Text {
          textFormat: Text.PlainText
          visible: root.mode === "keyboard"
          text: "←→ machine   1–9 workspace   ↵ give   T take back   W watch   F send files   G get files   S sync a folder   Y synced folders   Esc"
          color: root.foreground
          opacity: 0.6
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
