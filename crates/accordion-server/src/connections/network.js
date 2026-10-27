"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

let status = null;

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}

// Returns the response's status with its data, so callers can tell "not
// signed in" from other problems.
async function api(method, path, body) {
  let response;
  try {
    response = await fetch(path, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body && JSON.stringify(body),
    });
  } catch {
    throw new Error("Accordion didn't respond. Check that the server is on, then try again.");
  }
  const data = await response.json().catch(() => ({}));
  return { status: response.status, data };
}

// Such as "October 22, 2026 at 3:04 AM", in the reader's time zone.
function when(millis) {
  return new Date(millis).toLocaleString(undefined, { dateStyle: "long", timeStyle: "short" });
}

// Such as "October 22, 2026".
function dateOf(millis) {
  return new Date(millis).toLocaleDateString(undefined, { dateStyle: "long" });
}

function hourLabel(hour) {
  if (hour === 0) return "midnight";
  if (hour === 12) return "noon";
  return hour < 12 ? `${hour} a.m.` : `${hour - 12} p.m.`;
}

// "a", "a and b", or "a, b, and c".
function list(items) {
  if (items.length < 3) return items.join(" and ");
  return `${items.slice(0, -1).join(", ")}, and ${items.at(-1)}`;
}

// The last connection, such as "Last connected October 22, 2026 at 3:04 AM.
// It worked."
function lastLine(last, none) {
  if (!last) return none;
  if (last.ok) return `Last connected ${when(last.at)}. It worked.`;
  return `Last connected ${when(last.at)}. It didn't work. ${last.problem || ""}`.trim();
}

// One connection: what it's for, where it goes, how often, the last time,
// and its switch, if it has one.
//
// `entry.toggle` is { on, path, offLabel, onLabel, offHint } or absent, and
// `entry.fixed` explains why there's no switch.
function section(entry) {
  const item = document.createElement("section");
  const title = document.createElement("h2");
  title.textContent = entry.title;
  item.append(title);
  const line = (text, className) => {
    const paragraph = document.createElement("p");
    paragraph.textContent = text;
    if (className) paragraph.className = className;
    item.append(paragraph);
  };
  if (entry.toggle && !entry.toggle.on) {
    const off = document.createElement("p");
    const strong = document.createElement("strong");
    strong.textContent = "Turned off.";
    off.append(strong);
    item.append(off);
  }
  line(entry.purpose);
  line(entry.where);
  line(entry.often);
  line(entry.last);
  if (entry.fixed) line(entry.fixed, "hint");
  if (entry.toggle) {
    const toggle = entry.toggle;
    if (toggle.on) line(toggle.offHint, "hint");
    const button = document.createElement("button");
    button.type = "button";
    button.className = "secondary";
    button.textContent = toggle.on ? toggle.offLabel : toggle.onLabel;
    button.addEventListener("click", () => turn(button, toggle.path, !toggle.on));
    item.append(button);
  }
  return item;
}

function entries() {
  const all = [];
  const updates = status.updates;
  if (updates) {
    const github = updates.host === "github.com";
    all.push({
      title: "Update check",
      purpose: github
        ? "Checks for a newer release of Accordion, and downloads it when it's time to install. Like any download, GitHub sees your internet address and counts the download. Accordion sends nothing else."
        : "Checks for a newer release of Accordion, and downloads it when it's time to install. Accordion sends nothing about this server.",
      where: github
        ? "Connects to github.com, which sends downloads on to GitHub's download server."
        : `Connects to ${updates.host}.`,
      often: "Once a day, and when an update downloads.",
      last: lastLine(updates.last, "It hasn't connected yet."),
      toggle: {
        on: updates.on,
        path: "/api/network/updates",
        offLabel: "Turn off the update check",
        onLabel: "Turn on the update check",
        offHint:
          "Turned off, Accordion doesn't check for or download updates on its own. You can still check and install from the updates page, and Accordion connects only then.",
      },
    });
  }

  const address = status.address;
  if (address) {
    const entry = {
      title: "Keeping your server's name pointed at it",
      purpose:
        "Updates your server's name so it points at this server's address on your home network, so your devices can find it.",
      where: `Connects to ${address.host}, your DNS provider.`,
      often:
        "When this server's address on your home network changes. Accordion looks at its own address each minute without connecting anywhere.",
      last: lastLine(address.last, "It hasn't connected since the server started."),
    };
    if (address.off_by_setting) {
      entry.fixed =
        "Turned off by the ACCORDION_ADDRESS setting on the server. To turn it on, change that setting, then restart Accordion.";
    } else {
      entry.toggle = {
        on: address.on,
        path: "/api/network/address",
        offLabel: "Turn off address updates",
        onLabel: "Turn on address updates",
        offHint:
          "Turn this off only if this server's address on your home network never changes, such as when your router always gives it the same address. If the address changes while this is off, your devices stop finding the server.",
      };
    }
    all.push(entry);
  }

  const certificate = status.certificate;
  if (certificate) {
    let last = lastLine(certificate.last, "It hasn't connected since the server started.");
    if (certificate.expires_at) {
      last += ` The current certificate expires ${dateOf(certificate.expires_at)}.`;
    }
    all.push({
      title: "Certificate renewal",
      purpose: "Renews the certificate that lets your devices connect to the server securely.",
      where: `Connects to ${list([
        certificate.authority_host,
        certificate.provider_host,
        "your domain's nameservers",
      ])}, to prove the name is yours and see that the proof is live.`,
      often: "About once every two months.",
      last,
      fixed:
        "This can't be turned off yet. Without renewal, the certificate expires, and your devices can't connect to the server. A way to run without renewal, for servers kept off the internet, is planned.",
    });
  }

  for (const target of status.backups) {
    all.push({
      title: `Backups to ${target.name}`,
      purpose: "Sends encrypted backups of everything on this server to this location.",
      where: `Connects to ${target.host}.`,
      often: `Every night at ${hourLabel(status.backup_hour)}, plus a check each week and a restore test each month.`,
      last: lastLine(target.last, "It hasn't connected yet."),
      toggle: {
        on: target.on,
        path: `/api/network/backups/${target.id}`,
        offLabel: "Pause backups here",
        onLabel: "Resume backups here",
        offHint:
          "Paused, Accordion doesn't back up to this location, check it, or test restoring from it. The backups already there stay.",
      },
    });
  }
  return all;
}

function render() {
  $("sign-in").hidden = true;
  $("manage").hidden = false;
  $("connections").replaceChildren(...entries().map(section));
}

async function load() {
  try {
    const { status: code, data } = await api("GET", "/api/network");
    if (code === 401) {
      $("sign-in").hidden = false;
      $("manage").hidden = true;
      return;
    }
    if (code !== 200) throw new Error(data.error || PROBLEM);
    status = data;
    render();
  } catch (error) {
    showError(error.message);
  }
}

async function turn(button, path, on) {
  showError("");
  button.disabled = true;
  try {
    const { status: code, data } = await api("PUT", path, { on });
    if (code !== 204) throw new Error(data.error || PROBLEM);
    await load();
  } catch (error) {
    showError(error.message);
    button.disabled = false;
  }
}

$("sign-in").addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const button = event.submitter;
  button.disabled = true;
  try {
    const { status: code, data } = await api("POST", "/api/session", {
      username: $("username").value,
      password: $("password").value,
    });
    if (code !== 200) throw new Error(data.error || PROBLEM);
    $("password").value = "";
    await load();
  } catch (error) {
    showError(error.message);
  } finally {
    button.disabled = false;
  }
});

load();
