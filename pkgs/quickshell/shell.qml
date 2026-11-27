import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ShellRoot {
    id: root

    readonly property var c: ({ @colors@ })
    property bool open: false
    property var screen: null
    property var st: ({ wifi: {}, agents: [], tasks: [] })
    readonly property int busy: st.agents.filter(a => a.status === "busy").length

    onOpenChanged: if (open) {
        close.interval = 2000;
        close.restart();
    }

    function run(cmd) {
        Quickshell.execDetached(["sh", "-c", cmd]);
        refresh.restart();
    }

    component Label: Text {
        color: root.c.text
        font.family: "DejaVu Sans"
        font.pixelSize: 13
        elide: Text.ElideRight
        verticalAlignment: Text.AlignVCenter
    }

    component Icon: Label {
        font.family: "FiraCode Nerd Font"
        font.pixelSize: 16
        horizontalAlignment: Text.AlignHCenter
        Layout.preferredWidth: 20
    }

    component Card: Rectangle {
        default property alias content: inner.data
        Layout.fillWidth: true
        implicitHeight: inner.implicitHeight + 20
        radius: 14
        color: root.c.mantle
        ColumnLayout {
            id: inner
            anchors.fill: parent
            anchors.margins: 10
            spacing: 6
        }
    }

    component Header: RowLayout {
        property string icon
        property color tint
        property string label
        property string detail
        spacing: 8
        Icon {
            text: parent.icon
            color: parent.tint
        }
        Label {
            text: parent.label
            font.bold: true
            Layout.fillWidth: true
        }
        Label {
            text: parent.detail
            color: root.c.subtext0
        }
    }

    component Pill: Rectangle {
        property string icon
        property color tint
        property bool active: true
        property bool toggle: false
        signal clicked
        Layout.fillWidth: true
        implicitHeight: 38
        radius: height / 2
        color: area.containsMouse ? root.c.surface0 : active && toggle ? Qt.alpha(tint, 0.15) : root.c.mantle
        Behavior on color {
            ColorAnimation {
                duration: 120
            }
        }
        Icon {
            anchors.centerIn: parent
            text: parent.icon
            color: parent.active ? parent.tint : root.c.overlay0
            font.pixelSize: 18
        }
        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: parent.clicked()
        }
    }

    component Level: RowLayout {
        property string icon
        property color tint
        property int value
        property string cmd
        spacing: 8
        Icon {
            text: parent.icon
            color: parent.tint
        }
        Slider {
            id: slider
            Layout.fillWidth: true
            implicitHeight: 20
            from: 0
            to: 100
            value: parent.value
            onMoved: root.run(parent.cmd.replace("%v", Math.round(value)))
            background: Rectangle {
                x: slider.leftPadding
                y: slider.topPadding + slider.availableHeight / 2 - height / 2
                width: slider.availableWidth
                height: 5
                radius: 5
                color: root.c.crust
                Rectangle {
                    width: slider.visualPosition * parent.width
                    height: parent.height
                    radius: 5
                    color: slider.parent.tint
                }
            }
            handle: Rectangle {
                x: slider.leftPadding + slider.visualPosition * (slider.availableWidth - width)
                y: slider.topPadding + slider.availableHeight / 2 - height / 2
                width: 12
                height: 12
                radius: 6
                color: slider.parent.tint
                opacity: slider.hovered || slider.pressed ? 1 : 0
                Behavior on opacity {
                    NumberAnimation {
                        duration: 120
                    }
                }
            }
        }
        Label {
            text: parent.value + "%"
            color: root.c.subtext0
            horizontalAlignment: Text.AlignRight
            Layout.preferredWidth: 36
        }
    }

    Process {
        id: poll
        command: ["panel-status"]
        stdout: StdioCollector {
            onStreamFinished: root.st = JSON.parse(text)
        }
    }

    Timer {
        interval: 3000
        running: root.open
        repeat: true
        triggeredOnStart: true
        onTriggered: poll.running = true
    }

    Timer {
        id: refresh
        interval: 500
        onTriggered: poll.running = true
    }

    Variants {
        model: Quickshell.screens

        PanelWindow {
            required property var modelData
            screen: modelData
            anchors {
                top: true
                right: true
            }
            implicitWidth: 1
            implicitHeight: 1
            color: "transparent"
            exclusionMode: ExclusionMode.Ignore
            WlrLayershell.layer: WlrLayer.Overlay
            WlrLayershell.namespace: "hot-corner"

            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                onEntered: {
                    root.screen = modelData;
                    root.open = true;
                }
            }
        }
    }

    PanelWindow {
        visible: root.open || panel.opacity > 0
        screen: root.screen
        anchors {
            top: true
            right: true
        }
        margins {
            top: 5
            right: 5
        }
        implicitWidth: 380
        implicitHeight: body.implicitHeight + 24
        color: "transparent"
        exclusionMode: ExclusionMode.Ignore
        WlrLayershell.layer: WlrLayer.Overlay
        WlrLayershell.namespace: "control-panel"
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.OnDemand

        Timer {
            id: close
            onTriggered: root.open = false
        }

        Rectangle {
            id: panel
            width: parent.width
            height: parent.height
            y: root.open ? 0 : -12
            opacity: root.open ? 1 : 0
            radius: 18
            color: Qt.alpha(root.c.base, 0.95)
            border.color: root.c.surface0
            focus: true
            Keys.onEscapePressed: root.open = false
            Behavior on opacity {
                NumberAnimation {
                    duration: 150
                }
            }
            Behavior on y {
                NumberAnimation {
                    duration: 150
                    easing.type: Easing.OutCubic
                }
            }

            HoverHandler {
                id: hover
                onHoveredChanged: {
                    if (hovered) {
                        close.interval = 500;
                        close.stop();
                    } else {
                        close.restart();
                    }
                }
            }

            ColumnLayout {
                id: body
                anchors.fill: parent
                anchors.margins: 12
                spacing: 8

                RowLayout {
                    spacing: 6
                    Pill {
                        icon: root.st.awake ? "󰅶" : "󰾪"
                        tint: root.c.peach
                        active: root.st.awake ?? false
                        toggle: true
                        onClicked: root.run("stay-awake toggle; pkill -RTMIN+8 waybar")
                    }
                    Pill {
                        icon: root.st.wg ? "" : ""
                        tint: root.c.lavender
                        active: root.st.wg ?? false
                        toggle: true
                        onClicked: root.run("wireguard-toggle")
                    }
                    Pill {
                        icon: (root.st.wifi.state ?? "") === "routable" ? "󰤨" : "󰤭"
                        tint: root.c.green
                        active: (root.st.wifi.state ?? "") === "routable"
                        toggle: true
                        onClicked: root.run("rfkill toggle wifi")
                    }
                    Pill {
                        icon: root.st.bluetooth ? "󰂯" : "󰂲"
                        tint: root.c.blue
                        active: root.st.bluetooth ?? false
                        toggle: true
                        onClicked: root.run("rfkill toggle bluetooth")
                    }
                }

                Card {
                    Header {
                        icon: "󰖩"
                        tint: root.c.green
                        label: root.st.wifi.ssid || "Disconnected"
                        detail: root.st.wg ? "wg0" : "VPN off"
                    }
                    Level {
                        icon: root.st.muted ? "󰝟" : "󰕾"
                        tint: root.st.muted ? root.c.overlay0 : root.c.sky
                        value: root.st.volume ?? 0
                        cmd: "pactl set-sink-volume @DEFAULT_SINK@ %v%"
                    }
                    Level {
                        icon: "󰃠"
                        tint: root.c.yellow
                        value: root.st.brightness ?? 0
                        cmd: "brightnessctl -c backlight set %v%"
                    }
                }

                Card {
                    Header {
                        icon: root.busy > 0 ? "󰚩" : "󱙺"
                        tint: root.busy > 0 ? root.c.green : root.c.overlay1
                        label: "Agents"
                        detail: root.busy + "/" + root.st.agents.length
                    }
                    Repeater {
                        model: root.st.agents
                        RowLayout {
                            required property var modelData
                            spacing: 8
                            Icon {
                                text: "●"
                                font.pixelSize: 9
                                color: modelData.status === "busy" ? root.c.green : modelData.status === "waiting" ? root.c.peach : root.c.overlay0
                            }
                            Label {
                                text: modelData.title
                                color: modelData.status === "busy" ? root.c.text : root.c.subtext0
                                Layout.fillWidth: true
                            }
                        }
                    }
                }

                Card {
                    visible: root.st.tasks.length > 0
                    Header {
                        icon: "󰄲"
                        tint: root.c.mauve
                        label: "Tasks"
                        detail: root.st.tasks.length
                    }
                    Repeater {
                        model: root.st.tasks
                        RowLayout {
                            required property var modelData
                            spacing: 8
                            Icon {
                                text: modelData.status === "doing" ? "󰐊" : "󰄱"
                                font.pixelSize: 13
                                color: modelData.status === "doing" ? root.c.blue : root.c.overlay0
                            }
                            Label {
                                text: modelData.description
                                color: modelData.status === "doing" ? root.c.text : root.c.subtext0
                                Layout.fillWidth: true
                            }
                        }
                    }
                }

                RowLayout {
                    spacing: 6
                    Pill {
                        icon: "󰍁"
                        tint: root.c.yellow
                        onClicked: root.run("loginctl lock-session")
                    }
                    Pill {
                        icon: ""
                        tint: root.c.lavender
                        onClicked: root.run("systemctl suspend")
                    }
                    Pill {
                        icon: ""
                        tint: root.c.sapphire
                        onClicked: root.run("systemctl hibernate")
                    }
                    Pill {
                        icon: "󰗼"
                        tint: root.c.peach
                        onClicked: root.run("loginctl terminate-user $USER")
                    }
                    Pill {
                        icon: "󰜉"
                        tint: root.c.mauve
                        onClicked: root.run("reboot")
                    }
                    Pill {
                        icon: ""
                        tint: root.c.red
                        onClicked: root.run("poweroff")
                    }
                }
            }
        }
    }
}
