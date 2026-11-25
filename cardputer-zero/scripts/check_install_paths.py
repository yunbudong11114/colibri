#!/usr/bin/env python3
"""Validate an extracted Debian data tree against CardputerZero store path rules."""
import argparse
import configparser
from pathlib import Path
import re
import stat


def related(name, package):
    normalize = lambda value: re.sub(r"[-_.]", "", value).lower()
    return normalize(name).startswith(normalize(package))


def allowed(path, package):
    parts = path.split("/")
    if path.startswith("usr/share/APPLaunch/") or path.startswith("usr/share/doc/"):
        return True
    for prefix in ("lib/systemd/system/", "usr/lib/systemd/system/"):
        if path.startswith(prefix):
            name = path[len(prefix):]
            return "/" not in name and name.endswith(".service") and related(name, package)
    if len(parts) == 3 and parts[:2] == ["usr", "bin"]:
        return related(parts[2], package)
    for prefix in ("usr/share/", "usr/lib/", "opt/", "etc/", "var/lib/"):
        if path.startswith(prefix):
            tail = path[len(prefix):].split("/")
            if len(tail) > 1 and related(tail[0], package):
                return True
            if prefix == "etc/" and len(tail) == 1 and tail[0].endswith(".conf") and related(tail[0], package):
                return True
    for prefix in ("etc/default/", "etc/logrotate.d/", "usr/share/lintian/overrides/",
                   "usr/share/bash-completion/completions/", "usr/lib/tmpfiles.d/"):
        if path.startswith(prefix):
            name = path[len(prefix):]
            if "/" not in name and related(name, package):
                return prefix != "usr/lib/tmpfiles.d/" or name.endswith(".conf")
    if re.fullmatch(r"usr/share/man/man[0-9][^/]*/[^/]+\.[0-9][^/]*\.gz", path):
        return related(parts[-1], package)
    if path.startswith("usr/lib/udev/rules.d/"):
        name = parts[-1]
        return len(parts) == 5 and bool(re.fullmatch(r"[0-9]{2}-.+\.rules", name)) and related(name[3:], package)
    return False


def check(root, package):
    root = Path(root)
    problems = []
    desktops = []
    for item in sorted(root.rglob("*")):
        mode = item.lstat().st_mode
        if stat.S_ISDIR(mode):
            continue
        path = item.relative_to(root).as_posix()
        if not (stat.S_ISREG(mode) or stat.S_ISLNK(mode)):
            problems.append(path + ": unsupported device/file type")
        if mode & (stat.S_ISUID | stat.S_ISGID):
            problems.append(path + ": setuid/setgid forbidden")
        if not allowed(path, package):
            problems.append(path + ": path does not belong to package")
        if path.startswith("usr/share/APPLaunch/applications/") and path.endswith(".desktop"):
            desktops.append(item)
        if path.endswith(".service") and "/systemd/system/" in path:
            parser = configparser.ConfigParser(interpolation=None)
            parser.read(item)
            user = parser.get("Service", "User", fallback="").strip()
            if not user or user.lower() == "root" or user.isdigit() and int(user) == 0:
                problems.append(path + ": service must declare a non-root User")
        if path.startswith("usr/lib/udev/rules.d/"):
            text = "\n".join(line.split("#", 1)[0] for line in item.read_text().splitlines())
            if re.search(r"\b(?:RUN(?:\{[^}]*\})?|PROGRAM|IMPORT\{program\})\s*[+:]?=", text, re.I):
                problems.append(path + ": executable udev action forbidden")
    if not desktops:
        problems.append("missing APPLaunch .desktop entry")
    for desktop in desktops:
        parser = configparser.ConfigParser(interpolation=None)
        parser.read(desktop)
        for field in ("Exec", "Icon"):
            value = parser.get("Desktop Entry", field, fallback="")
            if not value.startswith("/") or not (root / value.lstrip("/")).is_file():
                problems.append(desktop.name + ": " + field + " must reference a packaged absolute path")
    return problems


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--package", required=True)
    args = parser.parse_args()
    issues = check(args.root, args.package)
    for issue in issues:
        print(issue)
    if issues:
        raise SystemExit(1)
    print("PASS: installation paths, file modes and APPLaunch references for " + args.package)
