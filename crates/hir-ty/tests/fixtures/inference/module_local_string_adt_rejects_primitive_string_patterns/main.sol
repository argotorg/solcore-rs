pragma solcore;

enum string {
    RuntimeString(word)
}

function inspect(value: string) returns (word) {
    match (value) {
        case "a" {
            return 1;
        }
        default {
            return 0;
        }
    }
}
