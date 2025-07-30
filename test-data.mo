import Result "mo:base/Result";
import OrderedMap "mo:base/OrderedMap";
import Nat "mo:base/Nat";

shared (msg) actor class() {
  func letElse() {
    let ?x = null else { return };
  };

  flexible let flexibleLet = 42;
  stable let stableLet = 42;

  public func hello() : async Text {};

  public shared(msg) func sharedCaller() {};
  public shared msg func sharedCallerVar() {};
  public shared ({ caller }) func sharedCallerPat() {};

  transient let transientSomething = flexibleLet + 42;
  transient let transientMap = OrderedMap.Make<Nat>(Nat.compare);
  transient let transientMap = OrderedMap.Make.Well<Nat>(Nat.compare);

  x := 1 + x;
  x := x + 2;
  x := x - 1;
  x := 1 - x;

  let _ = { field = field };
  let _ = switch _ {
    case (false) {};
    case (true) {};
  };
  let _ = switch _ {
    case true {};
    case false {};
  };
}
