import Result "mo:base/Result";
import OrderedMap "mo:base/OrderedMap";
import PureList "mo:core/pure/List";

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
  let _ = { var dontPun = dontPun };
  let _ = switch _ {
    case (false) {};
    case (true) {};
  };
  let _ = switch _ {
    case true {};
    case false {};
  };

  let _ = switch _ {
    case (true, _) {};
    case (_, true) {};
  };

  public func listReturningFunction() : async List<Text> {
    null
  };
  public func setReturningFunction() : async Set.Set<Text> {
    null
  };
  public func mapReturningFunction() : async Map.Map<Text, Nat> {
    null
  };
  public func arrayReturningFunction() : async [Text] {
    null
  };

  func neededReturn() {
    return 10;
    20
  };

  func unneededReturn() {
    return 10
  };
  func unneededReturn() = return 10;

  func unneededReturn() {
    if (true) {
      return 4;
    } else {
      return 5;
      20;
    };
  };
  func unneededReturn() {
    if (true) return 4
    else {
      return 5;
      20;
    };
  };

  func unneededReturn() {
    switch (true) {
      case 1 return 40;
      case 2 { return 40; };
    };
  };

  type lowerCase = Nat;
  type Snake_case = Nat;
  type CamelCase<lowerCase, Snake_case> = Nat;
  type CamelCase = Nat;

  func UpperCase() {};
  func snake_case() {};
  func _hiddenIsFine() {};
  class lowerCase() {};
  class Snake_Cased_ish() {};
  class CamelCase<lowerCase, Snake_case>() {};
  class CamelCase<Camel, Case>() {};

}
