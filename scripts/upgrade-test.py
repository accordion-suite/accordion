#!/usr/bin/env python3
"""Adds data to a test server and checks it after each update, for
scripts/upgrade-test.sh.

Usage: scripts/upgrade-test.py add <state file>
       scripts/upgrade-test.py check <state file> [<version>]
       scripts/upgrade-test.py update <state file> <version> <outcome> <installed version>

The server is a throwaway install that serves plain HTTP on 127.0.0.1:8080,
because it has an owner but no name. add signs in as the owner, which
upgrade-test.sh creates, then adds a person, files in Drive over WebDAV, and
a backup to a folder. It saves what it added, with the test passwords, in the
state file. check reads it all back, and runs a backup and a restore test.
update installs a release from the Updates page's API, waits for the update
helper, and checks how the update ended: installed or rolled-back.

Only the standard library, so it runs on a fresh machine.
"""

import base64
import hashlib
import http.client
import json
import os
import re
import sys
import time
import urllib.parse

HOST = "127.0.0.1"
PORT = 8080
OWNER = "alex"
# A throwaway server's password, which upgrade-test.sh gives create-owner.
PASSWORD = "correct horse"
BACKUP_FOLDER = "/mnt/accordion-upgrade-test"
# Releases from before decision 13 have no web app.
WEB_APP = "/usr/lib/accordion/web/index.html"
# The web app's list of its files, with their hashes. Older releases don't
# have it.
WEB_FILES = "/usr/lib/accordion/web/files.json"
# A non-ASCII name, so the test covers names that need encoding.
FILES = {
    "Documents/Café notes.txt": "Pick up the bread.\n".encode(),
    "Photos/beach.bin": os.urandom(3 * 1024 * 1024),
}
# The update helper waits up to 10 minutes for a new version to start, then
# five more to see that it stays up.
UPDATE_TIMEOUT = 30 * 60


def fail(message):
    sys.exit(f"upgrade test: {message}")


def say(message):
    print(f"upgrade test: {message}", flush=True)


def request(method, path, token=None, body=None, auth=None, raw=None):
    """Sends one request. Returns the status, the headers, and the body,
    decoded from JSON when it's JSON."""
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if auth:
        headers["Authorization"] = "Basic " + base64.b64encode(auth.encode()).decode()
    data = raw
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    connection = http.client.HTTPConnection(HOST, PORT, timeout=60)
    try:
        connection.request(method, urllib.parse.quote(path), body=data, headers=headers)
        response = connection.getresponse()
        content = response.read()
    finally:
        connection.close()
    if response.getheader("Content-Type", "").startswith("application/json"):
        content = json.loads(content)
    return response.status, response.headers, content


def call(method, path, token, body=None, expect=200):
    status, _, content = request(method, path, token=token, body=body)
    if status != expect:
        fail(f"{method} {path} answered {status}, not {expect}: {content}")
    return content


def wait_for_server(limit=180):
    """Waits until the server answers, such as after it restarts."""
    deadline = time.monotonic() + limit
    while True:
        try:
            request("GET", "/api/session")
            return
        except OSError:
            if time.monotonic() > deadline:
                fail(f"the server didn't answer on {HOST}:{PORT} within {limit} seconds.")
            time.sleep(2)


def sign_in():
    """Signs in as the owner, and returns the session token from the
    cookie. A new sign-in counts as recent, which some changes need."""
    wait_for_server()
    status, headers, content = request(
        "POST", "/api/session", body={"username": OWNER, "password": PASSWORD}
    )
    if status != 200:
        fail(f"signing in as {OWNER} answered {status}: {content}")
    cookie = headers.get("Set-Cookie", "")
    return cookie.split(";", 1)[0].split("=", 1)[1]


def wait_for_backups(token, done, what):
    """Polls the backups page's status until `done` says the work is over."""
    deadline = time.monotonic() + 600
    while True:
        status = call("GET", "/api/backups", token)
        if not status["running"] and done(status):
            return status
        if time.monotonic() > deadline:
            fail(f"{what} didn't finish within 10 minutes: {status}")
        time.sleep(2)


def back_up(token, target_id):
    """Runs a backup now, and checks that it worked."""
    # The server might be backing up on its own, such as at the nightly hour.
    before = target(wait_for_backups(token, lambda _: True, "The backup already running"), target_id)
    previous = (before["last_backup"] or {}).get("finished_at", 0)
    call("POST", "/api/backups/run", token, expect=202)
    status = wait_for_backups(
        token,
        lambda status: (target(status, target_id)["last_backup"] or {}).get("finished_at", 0)
        > previous,
        "The backup",
    )
    outcome = target(status, target_id)["last_backup"]
    if not outcome["ok"]:
        fail(f"the backup didn't work: {outcome['problem']}")


def target(status, target_id):
    for item in status["targets"]:
        if item["id"] == target_id:
            return item
    fail(f"the backup location {target_id} is gone: {status}")


def dav(method, path, password, raw=None, expect=(200, 201, 204)):
    status, _, content = request(
        method, f"/dav/drive/{path}", auth=f"{OWNER}:{password}", raw=raw
    )
    if status not in expect:
        fail(f"{method} /dav/drive/{path} answered {status}: {content[:200]!r}")
    return content


def add(state_file):
    token = sign_in()

    # The answer holds the link to set Sam's password, which the test doesn't
    # need.
    call(
        "POST",
        "/api/people",
        token,
        {"username": "sam", "display_name": "Sam Lee", "role": "user"},
        expect=201,
    )

    created = call(
        "POST",
        "/api/app-passwords",
        token,
        {"device_name": "Upgrade test", "use_for": "dav"},
        expect=201,
    )
    app_password = created["password"].replace(" ", "")
    for path, content in FILES.items():
        dav("MKCOL", path.split("/")[0], app_password, expect=(201, 405))
        dav("PUT", path, app_password, raw=content)

    key = call("POST", "/api/backups/key", token, expect=201)["recovery_key"]
    call("POST", "/api/backups/key/confirm", token, {"recovery_key": key}, expect=204)
    call(
        "POST",
        "/api/backups/targets",
        token,
        {"name": "Test disk", "kind": "folder", "path": BACKUP_FOLDER},
        expect=201,
    )
    target_id = call("GET", "/api/backups", token)["targets"][0]["id"]
    back_up(token, target_id)

    state = {
        "token": token,
        "app_password": app_password,
        "target": target_id,
        "files": {path: hashlib.sha256(content).hexdigest() for path, content in FILES.items()},
    }
    with open(state_file, "w") as file:
        json.dump(state, file)
    say("added a person, two files in Drive, and a backup.")


def check(state_file, version=None):
    with open(state_file) as file:
        state = json.load(file)
    wait_for_server()

    # The session from before the update still works.
    call("GET", "/api/session", state["token"])
    token = sign_in()
    if version:
        installed = call("GET", "/api/updates", token)["installed"]
        if installed != version:
            fail(f"the server says it's {installed}, not {version}.")

    if os.path.isfile(WEB_APP):
        check_web_app(token)

    usernames = {person["username"] for person in call("GET", "/api/people", token)["people"]}
    if usernames != {"alex", "sam"}:
        fail(f"the people are {sorted(usernames)}, not alex and sam.")

    for path, digest in state["files"].items():
        content = dav("GET", path, state["app_password"], expect=(200,))
        if hashlib.sha256(content).hexdigest() != digest:
            fail(f"{path} changed.")

    status = call("GET", "/api/backups", token)
    if status["key"] != "confirmed":
        fail(f"the recovery key is {status['key']}, not confirmed.")
    if not target(status, state["target"])["last_good_backup"]:
        fail("the backup from before the update is gone.")
    back_up(token, state["target"])
    status = call("GET", "/api/backups", token)
    before = (target(status, state["target"])["last_restore_test"] or {}).get("finished_at", 0)
    call("POST", f"/api/backups/targets/{state['target']}/test", token, expect=202)
    status = wait_for_backups(
        token,
        lambda status: (target(status, state["target"])["last_restore_test"] or {}).get(
            "finished_at", 0
        )
        > before,
        "The restore test",
    )
    outcome = target(status, state["target"])["last_restore_test"]
    if not outcome["ok"]:
        fail(f"the restore test didn't work: {outcome['problem']}")
    say(f"the data is all there{f' on {version}' if version else ''}, and backups work.")


def check_web_app(token):
    """Drive's address opens the installed web app, and its script loads.
    Where the release lists the app's files, each one the server sends
    matches the list, and Server health says the app has them all."""
    status, headers, page = request("GET", "/drive/")
    if status != 200 or not headers.get("Content-Type", "").startswith("text/html"):
        fail(f"Drive's page answered {status} with {headers.get('Content-Type')}, not the web app.")
    script = re.search(rb'src="(/app/[^"]+[.]js)"', page)
    if not script:
        fail("Drive's page doesn't load the web app's script.")
    path = script.group(1).decode()
    status, _, _ = request("GET", path)
    if status != 200:
        fail(f"the web app's script, {path}, answered {status}.")
    if not os.path.isfile(WEB_FILES):
        return
    with open(WEB_FILES) as file:
        files = json.load(file)["files"]
    for name, digest in files.items():
        # This client doesn't ask for compressed copies, so the server sends
        # each file in app/ as it is. Server health covers the rest.
        if not name.startswith("app/") or name.endswith((".gz", ".br")):
            continue
        status, _, content = request("GET", "/" + name)
        if status != 200 or hashlib.sha256(content).hexdigest() != digest:
            fail(f"the web app's {name} answered {status}, or doesn't match the release's list.")
    checks = call("GET", "/api/health", token)["checks"]
    web = next((check for check in checks if check["id"] == "web_app"), None)
    if not web or web["state"] != "ok":
        fail(f"Server health says this about the web app: {web}")


def update(state_file, version, outcome, installed):
    with open(state_file) as file:
        state = json.load(file)
    token = state["token"]
    wait_for_server()

    found = call("POST", "/api/updates/check", token)
    problem = (found["last_check"] or {}).get("problem")
    if problem:
        fail(f"the update check didn't work: {problem}")
    newest = (found["newest"] or {}).get("version")
    if newest != version or not found["can_install"]:
        fail(f"the server found {newest}, not {version} ready to install: {found}")

    started = time.time() * 1000
    call("POST", "/api/updates/install", token, expect=202)
    say(f"installing {version}. The update helper takes at least five minutes.")
    deadline = time.monotonic() + UPDATE_TIMEOUT
    while True:
        time.sleep(10)
        try:
            status, _, content = request("GET", "/api/updates", token=token)
        except OSError:
            # Stopped for the update.
            status = None
        if status == 200:
            reports = [
                report
                for report in content["history"]
                if report["version"] == version and report["finished_at"] >= started
            ]
            if reports:
                report = reports[0]
                break
        if time.monotonic() > deadline:
            fail(f"the update helper didn't finish within {UPDATE_TIMEOUT // 60} minutes.")

    if report["outcome"] != outcome:
        fail(f"the update to {version} {report['outcome']}, not {outcome}: {report['reason']}")
    if content["installed"] != installed:
        fail(f"after the update, the server is {content['installed']}, not {installed}.")
    reason = f" ({report['reason']})" if report["reason"] else ""
    say(f"the update to {version} {outcome.replace('-', ' ')}{reason}, and the server is {installed}.")


def main():
    match sys.argv[1:]:
        case ["add", state_file]:
            add(state_file)
        case ["check", state_file]:
            check(state_file)
        case ["check", state_file, version]:
            check(state_file, version)
        case ["update", state_file, version, outcome, installed]:
            update(state_file, version, outcome, installed)
        case _:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
