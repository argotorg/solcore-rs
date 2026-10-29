pragma solcore;

import { string } from std;

type Text = string;
type Big = integer;

function explicitlyNeedsText(comptime value: Text) returns () {
    return ();
}

function explicitlyNeedsBig(comptime value: Big) returns () {
    return ();
}

function textParamIsComptime(value: Text) returns () {
    return explicitlyNeedsText(value);
}

function bigParamIsComptime(value: Big) returns () {
    return explicitlyNeedsBig(value);
}

function takesText(value: Text) returns () {
    return ();
}

function takesBig(value: Big) returns () {
    return ();
}

function exerciseText(value: Text) returns (Text) {
    let copy: Text = value;
    takesText(copy);
    return copy;
}

function exerciseBig(value: Big) returns (Big) {
    let copy: Big = value;
    takesBig(copy);
    return copy;
}
