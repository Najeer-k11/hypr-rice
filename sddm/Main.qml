// rice — SDDM greeter that mirrors hyprlock.conf (clock, date, password pill)
// plus a session toggle (bottom-left) and power buttons (bottom-right).
import QtQuick

Rectangle {
    id: root
    width: 1920; height: 1080
    color: config.surface

    // hyprlock geometry is in logical px on a 864-px-tall screen; scale to ours
    readonly property real s: height / 864
    readonly property string font: "GeistMono Nerd Font"
    property int sessionIndex: sessionModel.lastIndex >= 0 ? sessionModel.lastIndex : 0
    property int userIndex: 0
    property var sessions: []
    property var users: []
    property string userName: users.length ? users[userIndex] : userModel.lastUser
    property bool failed: false
    property bool entering: false   // login sent: fade to the bright wallpaper
    property string battery: ""     // empty until read; needs QML_XHR_ALLOW_FILE_READ=1

    function readFile(path, done) {
        var x = new XMLHttpRequest()
        x.onreadystatechange = function() { if (x.readyState === XMLHttpRequest.DONE) done(x.responseText.trim()) }
        x.open("GET", "file://" + path)
        x.send()
    }
    function readBattery() {
        readFile("/sys/class/power_supply/BAT0/capacity", function(cap) {
            if (cap === "") return
            readFile("/sys/class/power_supply/BAT0/status", function(status) {
                var icons = ["󰂎", "󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"]
                var icon = status === "Charging" ? "󰂄" : icons[Math.floor(Math.min(parseInt(cap), 100) / 10)]
                root.battery = icon + "  " + cap + "%"
            })
        })
    }
    Component.onCompleted: readBattery()
    Timer { interval: 30000; running: true; repeat: true; onTriggered: root.readBattery() }
    // If the login neither fails nor starts, bring the form back.
    Timer { interval: 10000; running: root.entering; onTriggered: { root.entering = false; pw.forceActiveFocus() } }

    // collect session / user names out of the models
    Repeater { model: sessionModel; Item { Component.onCompleted: { var a = root.sessions.slice(); a[index] = name; root.sessions = a } } }
    Repeater { model: userModel; Item { Component.onCompleted: {
        var a = root.users.slice(); a[index] = name; root.users = a
        if (name === userModel.lastUser) root.userIndex = index } } }

    Connections {
        target: sddm
        function onLoginFailed() { root.entering = false; root.failed = true; pw.text = ""; pw.forceActiveFocus() }
        function onLoginSucceeded() { root.failed = false }
    }

    Image {
        anchors.fill: parent
        source: config.background
        fillMode: Image.PreserveAspectCrop
        asynchronous: false
        smooth: true
    }
    Rectangle {   // brightness = 0.55, lifted on login so the desktop's wallpaper follows seamlessly
        anchors.fill: parent; color: "black"
        opacity: root.entering ? 0 : 0.45
        Behavior on opacity { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
    }

    Item {
    id: form
    anchors.fill: parent
    opacity: root.entering ? 0 : 1
    Behavior on opacity { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }

    // ---- clock + date -----------------------------------------------------
    Text {
        id: clock
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        anchors.verticalCenterOffset: -140 * s
        color: config.onSurface
        font { family: root.font; bold: true; pixelSize: 84 * s * 1.33 }
        text: Qt.formatTime(new Date(), "HH:mm")
    }
    Text {
        id: date
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        anchors.verticalCenterOffset: -60 * s
        color: config.onSurfaceVariant
        font { family: root.font; pixelSize: 16 * s * 1.33 }
        text: Qt.formatDate(new Date(), "dddd, dd MMMM")
    }
    Timer { interval: 1000; running: true; repeat: true; onTriggered: {
        clock.text = Qt.formatTime(new Date(), "HH:mm"); date.text = Qt.formatDate(new Date(), "dddd, dd MMMM") } }

    // ---- password pill ----------------------------------------------------
    Rectangle {
        id: field
        width: 260 * s; height: 44 * s; radius: 6 * s
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        anchors.verticalCenterOffset: 40 * s
        color: config.surface
        border.width: 2 * s
        border.color: root.failed ? config.error : config.primary

        TextInput {
            id: pw
            anchors.fill: parent; anchors.leftMargin: 14 * s; anchors.rightMargin: 14 * s
            verticalAlignment: TextInput.AlignVCenter
            horizontalAlignment: TextInput.AlignHCenter
            color: config.onSurface
            font { family: root.font; pixelSize: 14 * s; letterSpacing: 4 * s }
            echoMode: TextInput.Password
            passwordCharacter: "●"
            focus: true
            enabled: !root.entering
            clip: true
            cursorDelegate: Rectangle { width: 2 * s; color: config.primary; opacity: pw.text.length ? 1 : 0 }
            onTextChanged: if (text.length) root.failed = false
            Keys.onReturnPressed: login()
            Keys.onEnterPressed: login()
            function login() { if (text.length) { root.entering = true; sddm.login(root.userName, text, root.sessionIndex) } }
        }
        Text {
            anchors.centerIn: parent
            visible: pw.text.length === 0
            color: root.failed ? config.error : config.onSurfaceVariant
            font { family: root.font; pixelSize: 14 * s; italic: root.failed }
            text: root.failed ? "authentication failed" : "password"
        }
    }

    // user (click to cycle when there are several)
    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: field.bottom; anchors.topMargin: 14 * s
        color: config.onSurfaceVariant
        opacity: 0.8
        font { family: root.font; pixelSize: 12 * s }
        text: "\uf007  " + root.userName
        MouseArea { anchors.fill: parent; enabled: root.users.length > 1; cursorShape: Qt.PointingHandCursor
            onClicked: { root.userIndex = (root.userIndex + 1) % root.users.length; pw.forceActiveFocus() } }
    }

    } // form

    // ---- bottom bar ---------------------------------------------------------
    component Pill: Rectangle {
        id: pill
        property alias label: t.text
        property real size: 13
        signal clicked()
        height: 34 * s; width: Math.max(height, t.implicitWidth + 28 * s); radius: 6 * s
        color: ma.containsMouse ? config.surfaceContainerHigh : Qt.rgba(0, 0, 0, 0.35)
        border.width: 1; border.color: ma.containsMouse ? config.primary : "transparent"
        Behavior on color { ColorAnimation { duration: 120 } }
        Text { id: t; anchors.centerIn: parent; color: ma.containsMouse ? config.primary : config.onSurface
               font { family: root.font; pixelSize: pill.size * s } }
        MouseArea { id: ma; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor
                    onClicked: { pill.clicked(); pw.forceActiveFocus() } }
    }

    Item {
    anchors.fill: parent
    opacity: form.opacity

    Pill {
        anchors { left: parent.left; bottom: parent.bottom; margins: 18 * s }
        label: "\uf108  " + (root.sessions[root.sessionIndex] || "session")
        visible: root.sessions.length > 0
        onClicked: root.sessionIndex = (root.sessionIndex + 1) % root.sessions.length
    }

    Row {
        anchors { right: parent.right; bottom: parent.bottom; margins: 18 * s }
        spacing: 8 * s
        Rectangle {   // battery: read-only
            visible: root.battery !== ""
            height: 34 * s; width: batteryText.implicitWidth + 28 * s; radius: 6 * s
            color: Qt.rgba(0, 0, 0, 0.35)
            Text { id: batteryText; anchors.centerIn: parent; color: config.onSurface
                   font { family: root.font; pixelSize: 13 * s }
                   text: root.battery }
        }
        Pill { label: "󰤄"; size: 18; visible: sddm.canSuspend;  onClicked: sddm.suspend() }
        Pill { label: "󰜉"; size: 18; visible: true;  onClicked: sddm.reboot() }
        Pill { label: "󰐥"; size: 18; visible: true; onClicked: sddm.powerOff() }
    }
    } // bottom bar
}
