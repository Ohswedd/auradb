"""Shared run-isolation helpers for the AuraDB Python conformance harnesses.

Every connector harness seeds fixed primary keys into named collections. Run two
copies of a harness against the same long-lived server and the second run would
collide on those keys: an ``insert`` of an already-present key raises, and exact
``count``/result assertions drift once a prior run's rows remain. These helpers
give each run its own collection namespace so repeated runs against one server are
safe by default, with no manual cleanup and no server-side change.

The isolation seam is the collection name. An AuraDB collection is identified over
the wire by its model's class name, so a per-run name prefix yields a fresh,
non-colliding keyspace. ``scoped_models`` returns run-scoped *subclasses* of the
declared models: a subclass inherits every field and rebuilds its schema under the
prefixed name, leaving the original model untouched. This relies only on the
long-stable model/collection contract, so it works identically under the current
Aura Connector and the published backward-compatibility connectors.

Defaults stay simple: with no flags the prefix is a fresh random token, so running
a harness twice in a row just works. Pass ``--run-id`` to pin a reproducible token
(handy for inspecting a specific run's collections) or ``--collection-prefix`` to
control the literal prefix outright. ``AURA_CONFORMANCE_RUN_ID`` in the environment
supplies a default ``--run-id`` so a whole suite of harnesses can share one run.
"""

from __future__ import annotations

import argparse
import os
import re
import secrets
from typing import TypeVar

ENV_RUN_ID = "AURA_CONFORMANCE_RUN_ID"

_SANITIZE = re.compile(r"[^0-9A-Za-z]+")

T = TypeVar("T", bound=type)


def add_isolation_args(parser: argparse.ArgumentParser) -> None:
    """Register the shared run-isolation flags on an argument parser."""
    group = parser.add_argument_group("run isolation")
    group.add_argument(
        "--run-id",
        default=None,
        help=(
            "Token that scopes this run's collections so repeated runs against the "
            "same server do not collide (default: a fresh random token; env "
            f"{ENV_RUN_ID} supplies a default)."
        ),
    )
    group.add_argument(
        "--collection-prefix",
        default=None,
        help=(
            "Literal prefix applied to every collection name, overriding --run-id. "
            "Use to pin an exact namespace, e.g. 'ci_'."
        ),
    )


def _sanitize(token: str) -> str:
    cleaned = _SANITIZE.sub("_", token).strip("_")
    return cleaned or "run"


def collection_prefix(args: argparse.Namespace) -> str:
    """Resolve the collection-name prefix for this run.

    Precedence: an explicit ``--collection-prefix`` (used verbatim) wins; otherwise
    a ``--run-id`` (or the ``AURA_CONFORMANCE_RUN_ID`` environment default) becomes
    ``<run-id>_``; otherwise a fresh random ``run<token>_`` keeps repeated runs
    isolated by default.
    """
    explicit = getattr(args, "collection_prefix", None)
    if explicit:
        return explicit
    run_id = getattr(args, "run_id", None) or os.environ.get(ENV_RUN_ID)
    if run_id:
        return f"{_sanitize(run_id)}_"
    return f"run{secrets.token_hex(4)}_"


def scope_name(prefix: str, name: str) -> str:
    """Apply the run prefix to a raw collection name (for non-model harnesses)."""
    return f"{prefix}{name}"


def scoped_models(prefix: str, *models: T) -> list[T]:
    """Return run-scoped subclasses of ``models`` whose collection names carry the prefix.

    Each returned class inherits all fields from its base and rebuilds its schema
    under ``<prefix><ClassName>``; the original model classes are left unchanged.
    """
    scoped: list[T] = []
    for model in models:
        scoped_name = f"{prefix}{model.__name__}"
        scoped.append(type(model)(scoped_name, (model,), {}))
    return scoped
