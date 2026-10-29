pragma solcore;

export { Point(*), Line(*) };

struct Point<T> {
    x: T;
    y: T;
}

struct Line<T> {
    start: Point<T>;
    end: Point<T>;
}
