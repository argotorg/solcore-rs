pragma solcore;

//~ specialize: loader bare
//~ specialize: return 3

enum Box<a> {
    Box(a)
}
trait StaticValue<a> {
    function take(comptime value: a) returns (word);
}
impl StaticValue<word> {
    function take(comptime value: word) returns (word) {
        return value;
    }
}
function get<a>(comptime value: Box<a>) returns (word) where a: StaticValue {
    match (value) {
        case (Box(inner)) {
            return StaticValue.take(inner);
        }
    }
}
function main() returns (word) {
    let value: word = 3;
    return get(Box(value));
}
