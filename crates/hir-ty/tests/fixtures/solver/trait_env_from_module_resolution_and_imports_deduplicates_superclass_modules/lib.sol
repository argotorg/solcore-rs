pragma solcore;

export { Parent, Child };

trait Parent<a> {}
trait Child<a> where a: Parent {}
