#!/usr/bin/env python3
"""Runs a command under strace and fails if anything it starts connects to
another machine.

Usage: scripts/check-connections.py cargo test --workspace --locked --offline

Tests use stand-ins on this machine for every outside service, so a connection
anywhere else means code reached out on its own, such as a dependency that
phones home. Decision 11 explains the check and what it allows.

Build first, such as with `cargo test --no-run`, and run the tests with
`--offline`, so cargo's own downloads aren't counted.
"""

import ipaddress
import os
import re
import subprocess
import sys
import tempfile

# Reserved for documentation, so nothing on the internet answers them. Tests
# use them to stand for a computer that doesn't answer.
DOCUMENTATION = [
    ipaddress.ip_network(net)
    for net in ("192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24", "2001:db8::/32")
]

# Multicast that routers never forward, so it stays on the home network,
# such as the mDNS announcements that let devices find the server.
LOCAL_NETWORK = [ipaddress.ip_network(net) for net in ("224.0.0.0/24", "ff02::/16")]

# Such as `1234<restic> connect(3, {sa_family=AF_INET, sin_port=htons(443),
# sin_addr=inet_addr("1.2.3.4")}, 16)`. sendto, sendmsg, and sendmmsg print
# the address the same way.
LINE = re.compile(r"^(\d+)(?:<([^>]*)>)? (\w+)\(")
ADDRESS = re.compile(
    r'htons\((\d+)\)[^}]*?(?:inet_addr\("([^"]+)"\)|inet_pton\(AF_INET6, "([^"]+)")'
)


def name_servers():
    servers = set()
    try:
        with open("/etc/resolv.conf") as conf:
            for line in conf:
                fields = line.split()
                if len(fields) >= 2 and fields[0] == "nameserver":
                    servers.add(ipaddress.ip_address(fields[1].split("%")[0]))
    except (OSError, ValueError):
        pass
    return servers


def allowed(address, port, servers):
    if address.is_loopback or address.is_unspecified:
        return True
    if any(address in net for net in DOCUMENTATION + LOCAL_NETWORK):
        return True
    # Tests look up names. Code that phones home still gets caught when it
    # connects to the address it looked up.
    return port == 53 and address in servers


def outside_connections(trace, servers):
    found = {}
    for line in trace:
        match = LINE.match(line)
        if not match:
            continue
        pid, program, call = match.groups()
        for port, v4, v6 in ADDRESS.findall(line):
            address = ipaddress.ip_address(v4 or v6)
            # Such as ::ffff:1.2.3.4, an IPv4 address on an IPv6 socket.
            if address.version == 6 and address.ipv4_mapped:
                address = address.ipv4_mapped
            if not allowed(address, int(port), servers):
                key = (program or f"process {pid}", str(address), int(port))
                found.setdefault(key, call)
    return found


def main():
    command = sys.argv[1:]
    if not command:
        sys.exit(__doc__.strip())
    with tempfile.TemporaryDirectory() as folder:
        log = os.path.join(folder, "trace")
        try:
            # -f follows the programs the command starts, such as restic and
            # ssh. --seccomp-bpf stops only on the traced calls, so the
            # tests run at close to full speed. -Y names the program or
            # thread, which for a Rust test is the start of the test's name.
            code = subprocess.call(
                ["strace", "-f", "--seccomp-bpf", "-Y", "-qq", "-s", "1",
                 "-e", "signal=none", "-e", "trace=connect,sendto,sendmsg,sendmmsg",
                 "-o", log, "--", *command]
            )
        except FileNotFoundError:
            sys.exit("strace isn't installed. Install it, such as with `sudo apt-get install strace`, then run this again.")
        with open(log, errors="replace") as trace:
            found = outside_connections(trace, name_servers())

    if found:
        print(
            f"\n{len(found)} connection(s) left this machine while the tests ran:\n",
            file=sys.stderr,
        )
        for (program, address, port), call in sorted(found.items()):
            shown = f"[{address}]" if ":" in address else address
            print(f"  {program} called {call} to {shown}:{port}", file=sys.stderr)
        print(
            "\nTests must use stand-ins on this machine for outside services. If a "
            "dependency update started this, find out why before accepting it. "
            "Decision 11 explains this check.",
            file=sys.stderr,
        )
        sys.exit(code or 1)
    sys.exit(code)


if __name__ == "__main__":
    main()
