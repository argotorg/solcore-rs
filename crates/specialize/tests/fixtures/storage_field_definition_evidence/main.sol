import { keepLibReachable } from lib;
import { keepCompetitorReachable } from competitor;

// This module intentionally has no local contract. The reachable contract
// main in lib is still a specialization root, while this module's trait env
// sees both the definition-side and competing CanStore instances.
