#!/usr/bin/env python3
"""Adds data to a test server and checks it after each update, for
scripts/upgrade-test.sh.

Usage: scripts/upgrade-test.py add <state file>
       scripts/upgrade-test.py check <state file> [<version> [documents]]
       scripts/upgrade-test.py update <state file> <version> <outcome> <installed version>

The server is a throwaway install that serves plain HTTP on 127.0.0.1:8080,
because it has an owner but no name. add signs in as the owner, which
upgrade-test.sh creates, then adds a person, files in Drive over WebDAV, and
a backup to a folder. It saves what it added, with the test passwords, in the
state file. check reads it all back, checks the thumbnails of a photo and a
video on releases that make them, and of a Word document when it's told to
with documents, and runs a backup and a restore test.
update installs a release from the Updates page's API, waits for the update
helper, and checks how the update ended: installed or rolled-back.

Only the standard library, so it runs on a fresh machine.
"""

import base64
import hashlib
import http.client
import io
import json
import os
import re
import sys
import time
import urllib.parse
import zipfile
import zlib

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


def png(width, height):
    """A small gradient as a PNG, for a file that gets a thumbnail."""

    def chunk(kind, data):
        return (
            len(data).to_bytes(4, "big")
            + kind
            + data
            + zlib.crc32(kind + data).to_bytes(4, "big")
        )

    # Each row starts with its filter type, 0 for none, then red, green,
    # and blue for each pixel.
    rows = b"".join(
        b"\0" + b"".join(bytes((x * 255 // width, y * 255 // height, 128)) for x in range(width))
        for y in range(height)
    )
    header = width.to_bytes(4, "big") + height.to_bytes(4, "big") + bytes([8, 2, 0, 0, 0])
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


# A two-second, 64 by 36 pixel H.264 video, for a file that gets a
# thumbnail, made with:
#
#   ffmpeg -f lavfi -i testsrc2=size=64x36:rate=1:duration=2 -c:v libx264 \
#       -preset veryslow -pix_fmt yuv420p -bsf:v filter_units=remove_types=6 \
#       -movflags +faststart -map_metadata -1 -fflags +bitexact \
#       -flags:v +bitexact clip.mp4
CLIP = base64.b64decode(
    "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"
)

def docx():
    """A one-paragraph Word document, for a file that gets a thumbnail
    through LibreOffice."""
    parts = {
        "[Content_Types].xml": (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" '
            'ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/word/document.xml" ContentType="application/'
            'vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
            "</Types>"
        ),
        "_rels/.rels": (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Target="word/document.xml" Type="http://schemas.'
            'openxmlformats.org/officeDocument/2006/relationships/officeDocument"/>'
            "</Relationships>"
        ),
        "word/document.xml": (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
            "<w:body><w:p><w:r><w:t>Dear Sam, the garden needs watering.</w:t></w:r></w:p>"
            "</w:body></w:document>"
        ),
    }
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, text in parts.items():
            # A fixed date, so the file is the same every run.
            archive.writestr(zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0)), text)
    return buffer.getvalue()


# A non-ASCII name, so the test covers names that need encoding.
FILES = {
    "Documents/Café notes.txt": "Pick up the bread.\n".encode(),
    "Documents/letter.docx": docx(),
    "Photos/beach.bin": os.urandom(3 * 1024 * 1024),
    "Photos/sunset.png": png(640, 480),
    "Videos/clip.mp4": CLIP,
}
# Releases with thumbnails (decision 14) make them within this many
# seconds.
THUMBNAIL_TIMEOUT = 120
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
        # The query, if any, is already encoded.
        connection.request(
            method, urllib.parse.quote(path, safe="/?=&"), body=data, headers=headers
        )
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
    say(f"added a person, {len(FILES)} files in Drive, and a backup.")


def check(state_file, version=None, documents=False):
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
    check_thumbnails(token, documents)

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


def check_thumbnails(token, documents):
    """On releases with thumbnails, the PNG and the video get one soon, and
    Drive sends them. Video thumbnails (decision 15) came in the same release
    as the others, and need ffmpeg, which the package recommends, so the
    install command installs it. With documents, the Word document gets one
    too, and its preview (decision 17), which needs LibreOffice."""
    checks = call("GET", "/api/health", token)["checks"]
    health = next((check for check in checks if check["id"] == "thumbnails"), None)
    if health is None:
        return
    icons = ["Videos show an icon"] + (["Office documents show an icon"] if documents else [])
    if health["state"] != "ok" or any(icon in health["summary"] for icon in icons):
        fail(f"Server health says this about thumbnails: {health['summary']}")
    names = ["sunset.png", "clip.mp4"] + (["letter.docx"] if documents else [])
    for name in names:
        deadline = time.monotonic() + THUMBNAIL_TIMEOUT
        while True:
            found = call("GET", f"/api/drive/search?q={name}", token)
            if len(found) != 1:
                fail(f"searching for {name} found {len(found)} files, not one.")
            if found[0]["thumbnail"]:
                break
            if time.monotonic() > deadline:
                fail(f"{name} had no thumbnail after {THUMBNAIL_TIMEOUT} seconds.")
            time.sleep(2)
        status, headers, content = request(
            "GET", f"/api/drive/items/{found[0]['id']}/thumbnail", token=token
        )
        if status != 200 or headers.get("Content-Type") != "image/webp" or content[8:12] != b"WEBP":
            fail(f"{name}'s thumbnail answered {status} with {headers.get('Content-Type')}.")
        if name.endswith(".docx"):
            if found[0]["preview"] != "document":
                fail(f"{name} has a thumbnail but no preview.")
            status, headers, content = request(
                "GET", f"/api/drive/items/{found[0]['id']}/preview", token=token
            )
            if (
                status != 200
                or headers.get("Content-Type") != "application/pdf"
                or not content.startswith(b"%PDF-")
            ):
                fail(f"{name}'s preview answered {status} with {headers.get('Content-Type')}.")


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
        case ["check", state_file, version, "documents"]:
            check(state_file, version, documents=True)
        case ["update", state_file, version, outcome, installed]:
            update(state_file, version, outcome, installed)
        case _:
            sys.exit(__doc__)


if __name__ == "__main__":
    main()
