pragma solcore;

export { uint256(*), Length };

struct uint256 {
    value: word;
}

trait Length<t> {
    function length(v: t) returns (uint256);
}
