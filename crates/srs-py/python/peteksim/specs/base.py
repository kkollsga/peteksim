"""The spec value-object foundation — house-style §API-consistency-contract.

A **spec** is a declarative, immutable value: it says WHAT (or, for a settings
object, HOW), holds NAMES not project objects (resolved at apply time), and
carries no compute. Every spec here supports the family conformance battery
(testing-doctrine R7): dict round-trip with a ``"spec"`` type tag, value equality
(+hash), ``.replace()`` derivation, and a domain-table ``repr``.

The compute stays in the Rust ``peteksim._core`` engine; a spec is applied by the
``peteksim.apply`` driver, which resolves its names against a loaded project and
calls the engine.
"""

from __future__ import annotations

import dataclasses
import fnmatch
import json
from typing import Any, ClassVar, Dict, List, Tuple, Type

# The spec registry — tag → class — populated by ``@spec``. Drives ``from_dict``
# dispatch AND the R7 conformance battery's completeness check (every exported
# spec type must be registered).
_REGISTRY: Dict[str, Type["Spec"]] = {}


def spec(cls: Type["Spec"]) -> Type["Spec"]:
    """Register a spec class under its ``_tag`` (default: the class name)."""
    tag = getattr(cls, "_tag", None) or cls.__name__
    cls._tag = tag  # type: ignore[attr-defined]
    if tag in _REGISTRY and _REGISTRY[tag] is not cls:
        raise RuntimeError(f"duplicate spec tag {tag!r}")
    _REGISTRY[tag] = cls
    return cls


def registered_specs() -> Tuple[Type["Spec"], ...]:
    """Every registered spec class (the conformance battery iterates this)."""
    return tuple(_REGISTRY.values())


class NotYetSupported(NotImplementedError):
    """A spec field is valid and serializes, but the engine capability that would
    honour it has not landed yet. Raised LOUDLY at apply time, naming the task
    that carries the capability — never a silent no-op."""


class ApplyError(ValueError):
    """A spec could not be resolved against the project at apply time (a missing
    name, an illegal combination). The message names BOTH the project object and
    the spec entry."""


def match_glob(pattern: str, name: str) -> bool:
    """Case-sensitive glob match (``fnmatch``); an exact name matches itself."""
    return pattern == name or fnmatch.fnmatchcase(name, pattern)


# --- serialization helpers ---------------------------------------------------

def _encode(v: Any) -> Any:
    if isinstance(v, Spec):
        return v.to_dict()
    if isinstance(v, tuple):
        return [_encode(x) for x in v]
    if isinstance(v, list):
        return [_encode(x) for x in v]
    if isinstance(v, dict):
        return {str(k): _encode(x) for k, x in v.items()}
    return v


def _decode(v: Any) -> Any:
    if isinstance(v, dict) and "spec" in v:
        return spec_from_dict(v)
    if isinstance(v, list):
        return [_decode(x) for x in v]
    if isinstance(v, dict):
        return {k: _decode(x) for k, x in v.items()}
    return v


def spec_from_dict(d: Dict[str, Any]) -> "Spec":
    """Reconstruct any spec from its tagged dict (the family ``from_dict``)."""
    if not isinstance(d, dict) or "spec" not in d:
        raise ValueError("not a spec dict (missing 'spec' type tag)")
    tag = d["spec"]
    cls = _REGISTRY.get(tag)
    if cls is None:
        raise ValueError(f"unknown spec tag {tag!r}")
    return cls.from_dict(d)


# --- table repr helpers ------------------------------------------------------

def render_table(title: str, headers: List[str], rows: List[List[Any]]) -> str:
    """A compact fixed-width table (the domain repr). Every field named."""
    cols = [str(h) for h in headers]
    body = [[("" if c is None else str(c)) for c in r] for r in rows]
    widths = [len(h) for h in cols]
    for r in body:
        for i, c in enumerate(r):
            widths[i] = max(widths[i], len(c))
    line = "  ".join(h.ljust(widths[i]) for i, h in enumerate(cols))
    sep = "  ".join("-" * widths[i] for i in range(len(cols)))
    out = [title, line, sep]
    for r in body:
        out.append("  ".join(c.ljust(widths[i]) for i, c in enumerate(r)))
    return "\n".join(out)


# --- the Spec base -----------------------------------------------------------

@dataclasses.dataclass(frozen=True, eq=False, repr=False)
class Spec:
    """Immutable declarative value with family value-semantics.

    Concrete specs are ``@dataclass(frozen=True, eq=False)`` subclasses decorated
    with ``@spec``; store collection data as **tuples** (immutable + JSON-able).
    Equality + hash are canonical over ``to_dict`` so a spec is comparable and
    hashable regardless of its field types.
    """

    _tag: ClassVar[str] = "Spec"

    # -- value semantics ------------------------------------------------------
    def to_dict(self) -> Dict[str, Any]:
        """A plain JSON-able dict carrying the ``"spec"`` type tag. A scenario is
        a durable file: removing/renaming a field is a CHANGELOG-gated break."""
        out: Dict[str, Any] = {"spec": self._tag}
        for f in dataclasses.fields(self):
            out[f.name] = _encode(getattr(self, f.name))
        return out

    @classmethod
    def from_dict(cls, d: Dict[str, Any]) -> "Spec":
        if d.get("spec") != cls._tag:
            raise ValueError(f"{cls.__name__}.from_dict: wrong tag {d.get('spec')!r}")
        kw = {}
        for f in dataclasses.fields(cls):
            if f.name in d:
                val = _decode(d[f.name])
                if isinstance(val, list):
                    val = tuple(val)
                kw[f.name] = val
        return cls(**kw)

    def __eq__(self, other: Any) -> bool:
        return isinstance(other, Spec) and self.to_dict() == other.to_dict()

    def __hash__(self) -> int:
        return hash(json.dumps(self.to_dict(), sort_keys=True, default=str))

    def replace(self, *args: Any, **changes: Any) -> "Spec":
        """Return a NEW value with field ``changes`` (the original is unchanged).

        Collection specs (Horizons, Layering, Contacts, ...) override to accept a
        leading name/glob targeting matching entries — ``hz.replace("H1",
        surface=...)``, ``lay.replace("Z*", dz=0.5)``."""
        if args:
            raise TypeError(
                f"{type(self).__name__}.replace takes no positional target; "
                f"pass field=value keyword changes"
            )
        return dataclasses.replace(self, **changes)

    # -- table repr -----------------------------------------------------------
    def _table(self) -> str:
        rows = [[f.name, _encode(getattr(self, f.name))] for f in dataclasses.fields(self)]
        return render_table(type(self).__name__, ["field", "value"], rows)

    def __repr__(self) -> str:
        return self._table()
