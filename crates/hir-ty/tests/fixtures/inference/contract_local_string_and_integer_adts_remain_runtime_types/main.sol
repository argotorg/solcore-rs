pragma solcore;

contract RuntimeNames {
    enum string {
        RuntimeString(word)
    }
    enum integer {
        RuntimeInteger(word)
    }

    function takesString(value: string) returns () {
        return ();
    }

    function takesInteger(value: integer) returns () {
        return ();
    }

    function exercise(value: word) returns () {
        takesString(string.RuntimeString(value));
        takesInteger(integer.RuntimeInteger(value));
        return ();
    }
}
