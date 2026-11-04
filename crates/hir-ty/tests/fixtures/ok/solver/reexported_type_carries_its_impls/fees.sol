pragma solcore;

export { Fee };

trait Fee<t> {
    function fee(x: t) returns (word);
}
