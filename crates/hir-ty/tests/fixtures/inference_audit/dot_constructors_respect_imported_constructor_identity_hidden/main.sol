pragma solcore;

import * from lib;
enum Decoy {
    Hidden(word)
}
function forge() returns (Secret) {
    return .Hidden(7);
}
function inspect(x: Secret) returns (word) {
    match (x) {
        case .Hidden(n) {
            return n;
        }
        case _ {
            return 0;
        }
    }
}

//~ loader: project
//~ infer: forge solve contains-code=SC0224
//~ infer: inspect solve contains-code=SC0224
