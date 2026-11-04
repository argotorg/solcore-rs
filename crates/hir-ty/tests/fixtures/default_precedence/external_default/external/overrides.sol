pragma solcore;

import * from std;
export { Ext(*) };
enum Ext<a> {
    Ext(a)
}
default impl<a> Choice<Ext<a>> {}
