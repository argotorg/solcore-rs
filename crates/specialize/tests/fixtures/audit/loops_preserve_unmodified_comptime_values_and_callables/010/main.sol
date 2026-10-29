pragma solcore;

//~ specialize: loader std-current
//~ specialize: frontend clean
//~ specialize: entry-return 7

function main(flag: bool) returns (word) {
    let get: function () returns (word) = lam () -> word {
        return 7;
    };
    for (; flag;) {
        match (0) {
            case seed {
                assembly {
                    seed := 1
                }
            }
        }
        break;
    }
    return get();
}
