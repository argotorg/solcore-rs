import { Marker, Box, Phantom } from lib;

// The derived instance is declared in an imported module and recursively
// discharges the class constraint for every declared type parameter.
function markBox(x: Box<word>) returns (word) {
    return Marker.mark(x);
}

function markPhantom<a>(x: Phantom<a>) returns (word) where a: Marker {
    return Marker.mark(x);
}
