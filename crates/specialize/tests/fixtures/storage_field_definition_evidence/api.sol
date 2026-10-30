pragma solcore;

// Re-export only the type/class surface. Reference edges do not import std's
// instances into competitor, so its local CanStore instance is unique there.
export std.{ storage(*), uint256(*), CanStore };
