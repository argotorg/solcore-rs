pragma solcore;

import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

trait Toggle<a> {
    function toggle(value: a) returns (a);
}

enum Switch {
    Off,
    On
}

impl Toggle<Switch> {
    function toggle(value: Switch) returns (Switch) {
        match (value) {
            case Switch.Off {
                return Switch.On;
            }
            case Switch.On {
                return Switch.Off;
            }
        }
    }
}

enum Door {
    Closed,
    Open
}

impl Toggle<Door> {
    function toggle(value: Door) returns (Door) {
        match (value) {
            case Door.Closed {
                return Door.Open;
            }
            case Door.Open {
                return Door.Closed;
            }
        }
    }
}

contract LightSwitch {
    state: Switch;
    door: Door;

    constructor() {
        state = Switch.Off;
        door = Door.Closed;
    }

    // #[send()]
    function flip() public {
        state = Toggle.toggle(state);
    }

    // #[() -> true]
    function isOn() public returns (bool) {
        match (state) {
            case Switch.On {
                return true;
            }
            default {
                return false;
            }
        }
    }

    // #[send()]
    function flipDoor() public {
        door = Toggle.toggle(door);
    }

    // #[() -> true]
    function isOpen() public returns (bool) {
        match (door) {
            case Door.Open {
                return true;
            }
            default {
                return false;
            }
        }
    }
}
