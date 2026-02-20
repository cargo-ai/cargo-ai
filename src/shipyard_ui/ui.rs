slint::slint! {
    import { Button, LineEdit, TextEdit } from "std-widgets.slint";

    export component ShipyardWindow inherits Window {
        title: "Cargo AI Shipyard";
        width: 1180px;
        height: 760px;
        background: rgb(239, 239, 242);

        in-out property <string> account_status_text;
        in-out property <string> last_command;
        in-out property <string> terminal_output;
        in-out property <string> status_label;
        in-out property <int> status_kind;
        in-out property <bool> command_running;
        in-out property <bool> account_ready;

        in-out property <string> account_email;
        in-out property <string> account_code;
        in-out property <bool> execution_view_visible;

        callback run_status();
        callback run_register(string);
        callback run_confirm(string);
        callback toggle_execution_view();

        private property <length> title_height: 54px;
        private property <length> shell_gap: 10px;
        private property <length> execution_panel_expanded_height: 280px;
        private property <length> execution_panel_collapsed_height: 52px;
        private property <length> execution_panel_height: root.execution_view_visible ? execution_panel_expanded_height : execution_panel_collapsed_height;
        private property <length> workspace_top: title_height + shell_gap;
        private property <length> workspace_height: root.height - title_height - execution_panel_height - shell_gap - shell_gap;

        private property <brush> status_color:
            root.status_kind == 1 ? rgb(30, 163, 74) :
            root.status_kind == 2 ? rgb(48, 125, 71) :
            root.status_kind == 3 ? rgb(177, 63, 63) :
            root.status_kind == 4 ? rgb(177, 63, 63) : rgb(109, 116, 128);

        Rectangle {
            x: 0px;
            y: 0px;
            width: parent.width;
            height: root.title_height;
            background: rgb(18, 114, 187);

            Image {
                x: 12px;
                y: 13px;
                width: 28px;
                height: 28px;
                source: @image-url("assets/cai-logo-2.png");
                image-fit: contain;
            }

            Text {
                x: 48px;
                y: 12px;
                text: "Cargo AI Shipyard";
                color: rgb(255, 255, 255);
                font-size: 22px;
                font-weight: 600;
            }

            Text {
                x: parent.width - self.width - 14px;
                y: 16px;
                text: root.status_label;
                color: root.status_color;
                font-size: 16px;
                font-weight: 700;
            }
        }

        Rectangle {
            x: 12px;
            y: root.workspace_top;
            width: parent.width - 24px;
            height: root.workspace_height;
            background: rgb(247, 247, 249);
            border-width: 1px;
            border-color: rgb(210, 211, 216);
            border-radius: 12px;
            animate height { duration: 170ms; easing: ease-out; }

            Rectangle {
                visible: root.account_ready;
                width: parent.width;
                height: parent.height;
                background: rgb(248, 248, 251);

                Image {
                    x: (parent.width - 240px) / 2;
                    y: (parent.height - 240px) / 2;
                    width: 240px;
                    height: 240px;
                    source: @image-url("assets/cai-logo-2-bw.png");
                    image-fit: contain;
                    opacity: 0.20;
                }

                Text {
                    x: 24px;
                    y: 24px;
                    text: "Workspace";
                    color: rgb(45, 50, 56);
                    font-size: 26px;
                    font-weight: 650;
                }

                Text {
                    x: 24px;
                    y: 64px;
                    text: "Authenticated. Add next Shipyard workflow panels here.";
                    color: rgb(91, 97, 108);
                    font-size: 20px;
                }
            }

            Rectangle {
                visible: !root.account_ready;
                x: 24px;
                y: 20px;
                width: parent.width - 48px;
                height: parent.height - 40px;
                background: rgb(246, 246, 249);

                Text {
                    x: 0px;
                    y: 0px;
                    text: "Account Setup";
                    color: rgb(42, 47, 55);
                    font-size: 34px;
                    font-weight: 700;
                }

                Text {
                    x: 0px;
                    y: 44px;
                    text: root.account_status_text;
                    color: rgb(177, 54, 54);
                    font-size: 18px;
                    font-weight: 600;
                }

                Text {
                    x: 0px;
                    y: 74px;
                    text: "Set up your Cargo AI account directly from Shipyard using explicit CLI flows.";
                    color: rgb(93, 101, 112);
                    font-size: 14px;
                }

                Rectangle {
                    x: 0px;
                    y: 106px;
                    width: parent.width;
                    height: 1px;
                    background: rgb(212, 215, 222);
                }

                Text {
                    x: 0px;
                    y: 124px;
                    text: "1) Register Email";
                    color: rgb(31, 34, 40);
                    font-size: 18px;
                    font-weight: 640;
                }

                LineEdit {
                    x: 0px;
                    y: 152px;
                    width: parent.width;
                    height: 34px;
                    text <=> root.account_email;
                    placeholder-text: "you@example.com";
                    enabled: !root.command_running;
                    font-size: 14px;
                }

                Button {
                    x: 0px;
                    y: 192px;
                    text: "Run account register";
                    enabled: !root.command_running && root.account_email != "";
                    clicked => {
                        root.run_register(root.account_email);
                    }
                }

                Text {
                    x: 0px;
                    y: 236px;
                    text: "2) Confirm Code";
                    color: rgb(31, 34, 40);
                    font-size: 18px;
                    font-weight: 640;
                }

                LineEdit {
                    x: 0px;
                    y: 264px;
                    width: parent.width;
                    height: 34px;
                    text <=> root.account_code;
                    placeholder-text: "temporary code from email";
                    enabled: !root.command_running;
                    font-size: 14px;
                }

                Button {
                    x: 0px;
                    y: 304px;
                    text: "Run account confirm";
                    enabled: !root.command_running && root.account_code != "";
                    clicked => {
                        root.run_confirm(root.account_code);
                    }
                }

                Text {
                    x: 0px;
                    y: 348px;
                    text: "3) Verify Session";
                    color: rgb(31, 34, 40);
                    font-size: 18px;
                    font-weight: 640;
                }

                Button {
                    x: 0px;
                    y: 378px;
                    text: "Run account status";
                    enabled: !root.command_running;
                    clicked => {
                        root.run_status();
                    }
                }
            }
        }

        Rectangle {
            x: 12px;
            y: root.height - root.execution_panel_height - 10px;
            width: parent.width - 24px;
            height: root.execution_panel_height;
            background: rgb(20, 23, 29);
            border-radius: 10px;
            animate y { duration: 170ms; easing: ease-out; }
            animate height { duration: 170ms; easing: ease-out; }

            Text {
                x: 12px;
                y: 10px;
                text: "Execution Feed";
                color: rgb(216, 222, 234);
                font-size: 17px;
                font-weight: 600;
            }

            Text {
                x: 140px;
                y: 10px;
                text: root.status_label;
                color: root.status_color;
                font-size: 16px;
                font-weight: 700;
            }

            if root.execution_view_visible: Button {
                x: parent.width - self.width - 56px;
                y: 6px;
                text: "Run account status";
                enabled: !root.command_running;
                clicked => {
                    root.run_status();
                }
            }

            toggle_shell := Rectangle {
                x: parent.width - 44px;
                y: 7px;
                width: 32px;
                height: 28px;
                private property <bool> is_hovered: toggle_touch.has-hover;
                border-radius: 14px;
                border-width: 1px;
                border-color: rgb(150, 157, 168);
                background: toggle_touch.pressed ? rgb(208, 212, 219) : toggle_shell.is_hovered ? rgb(232, 236, 243) : rgb(224, 229, 236);

                Text {
                    x: (parent.width - self.width) / 2;
                    y: 3px;
                    text: ">";
                    color: rgb(55, 61, 72);
                    font-size: 18px;
                    font-weight: 700;
                }

                toggle_touch := TouchArea {
                    width: parent.width;
                    height: parent.height;
                    mouse-cursor: pointer;
                    clicked => {
                        root.toggle_execution_view();
                    }
                }
            }

            if toggle_shell.is_hovered: Rectangle {
                x: toggle_shell.x - 180px;
                y: 7px;
                width: 172px;
                height: 28px;
                border-radius: 14px;
                border-width: 1px;
                border-color: rgb(173, 178, 186);
                background: rgb(243, 245, 249);

                Text {
                    x: 14px;
                    y: 5px;
                    text: "Toggle execution view";
                    color: rgb(39, 43, 50);
                    font-size: 13px;
                    font-weight: 520;
                }
            }

            if root.execution_view_visible: Text {
                x: 12px;
                y: 66px;
                text: root.last_command == "" ? "Command: (none yet)" : "Command: " + root.last_command;
                color: rgb(184, 192, 205);
                font-size: 13px;
            }

            if root.execution_view_visible: Rectangle {
                x: 10px;
                y: 90px;
                width: parent.width - 20px;
                height: parent.height - 100px;
                background: rgb(15, 18, 23);
                border-width: 1px;
                border-color: rgb(47, 55, 67);
                border-radius: 8px;

                TextEdit {
                    x: 8px;
                    y: 8px;
                    width: parent.width - 16px;
                    height: parent.height - 16px;
                    text: root.terminal_output == "" ? "No command output yet." : root.terminal_output;
                    font-size: 13px;
                    font-family: "Menlo";
                    read-only: true;
                    wrap: word-wrap;
                }
            }
        }
    }
}
