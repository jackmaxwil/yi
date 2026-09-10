#!/usr/bin/env python3
"""Write ~/.yi/oauth/<provider>.json (D172).

Yi ships no provider identity. This script is the scaffold that writes the profile
file; the four discover_* functions below are stubs that return REPLACE_ME. Fill in
whatever source you want them to read and run it -- the plumbing here (paths, shape,
0600, atomic replace, validation) is done.

Nothing in this repo supplies client ids, endpoints, or wire identity, and nothing
here reads another program's data. That is the part you own.

    ./scripts/oauth-profile-gen.py <provider>          # write the profile
    ./scripts/oauth-profile-gen.py <provider> --print  # to stdout, write nothing
"""

import json
import os
import pathlib
import sys
import tempfile

PLACEHOLDER = "REPLACE_ME"


def discover_endpoints(provider: str) -> dict:
    """Return {"authorize": str, "token": str, "scopes": str} for `provider`."""
    # TODO(you): return the provider's real endpoints and scopes.
    return {
        "authorize": f"https://{PLACEHOLDER}/oauth/authorize",
        "token": f"https://{PLACEHOLDER}/oauth/token",
        "scopes": PLACEHOLDER,
    }


def discover_client(provider: str) -> dict:
    """Return {"client_id": str} and optionally {"client_secret": str}."""
    # TODO(you): return the client id this login should present.
    return {"client_id": PLACEHOLDER}


def discover_stream_headers(provider: str) -> dict:
    """Return header-name -> value sent on every request for `provider`.

    Merged over the adapter's own headers: a name replaces, "" removes.
    """
    # TODO(you): return the wire identity headers.
    return {"user-agent": PLACEHOLDER}


def discover_refresh_headers(provider: str) -> dict:
    """Return header-name -> value sent only on the token-refresh call."""
    # TODO(you): return the refresh-call headers, or {} for none.
    return {}


def discover_callback(provider: str) -> dict:
    """Return the loopback redirect the provider has registered for this client."""
    # TODO(you): return the port/path/host the provider will redirect to.
    return {
        "callback_port": 8765,
        "callback_path": "/callback",
        "redirect_host": "localhost",
        "port_fallback": True,
        "json_token": True,
    }


def build(provider: str) -> dict:
    profile = {"kind": "oauth-code"}
    profile.update(discover_client(provider))
    profile.update(discover_endpoints(provider))
    profile.update(discover_callback(provider))
    profile["extra_authorize"] = {}
    profile["stream_headers"] = discover_stream_headers(provider)
    profile["refresh_headers"] = discover_refresh_headers(provider)
    return profile


REQUIRED = ("client_id", "authorize", "token", "callback_port")


def unfilled(profile: dict) -> list:
    """Every field still carrying the placeholder, so a half-filled profile is loud."""
    holes = [key for key in REQUIRED if PLACEHOLDER in str(profile.get(key, ""))]
    for group in ("stream_headers", "refresh_headers"):
        holes += [
            f"{group}.{name}"
            for name, value in profile.get(group, {}).items()
            if PLACEHOLDER in str(value)
        ]
    return holes


def path_for(provider: str) -> pathlib.Path:
    safe = "".join(ch if ch.isalnum() or ch in "-_" else "_" for ch in provider)
    return pathlib.Path.home() / ".yi" / "oauth" / f"{safe}.json"


def write(path: pathlib.Path, profile: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, tmp = tempfile.mkstemp(dir=path.parent)
    with os.fdopen(handle, "w") as out:
        json.dump(profile, out, indent=2)
        out.write("\n")
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def main(argv: list) -> int:
    args = [a for a in argv[1:] if not a.startswith("-")]
    if len(args) != 1:
        print(__doc__.strip().splitlines()[-2].strip(), file=sys.stderr)
        return 2
    provider = args[0]
    profile = build(provider)
    holes = unfilled(profile)

    if "--print" in argv[1:]:
        json.dump(profile, sys.stdout, indent=2)
        print()
    else:
        path = path_for(provider)
        write(path, profile)
        print(f"wrote {path}")

    if holes:
        print(
            f"warning: {len(holes)} field(s) still {PLACEHOLDER}: {', '.join(holes)}\n"
            f"         fill the discover_* stubs in {__file__}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
