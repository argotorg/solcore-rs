function nested(p: ((), (bool, word))) returns (word) {
    match (p) {
        case ((), (true, x)) {
            return x;
        }
    }
}
