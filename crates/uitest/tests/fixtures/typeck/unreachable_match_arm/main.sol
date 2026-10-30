pragma solcore;

enum Flag {
    Off,
    On
}

function pick(x: Flag) returns (word) {
    match (x) {
        case _ {
            return 0;
        }
        case Flag.Off {
            return 1;
        }
    }
}
