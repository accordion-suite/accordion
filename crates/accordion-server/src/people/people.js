"use strict";

const $ = (id) => document.getElementById(id);

const PROBLEM = "Accordion ran into a problem. Try again in a moment.";

const ROLES = {
  owner: "Owner",
  admin: "Admin",
  user: "Standard account",
};

// What to do once the admin confirms their password.
let afterConfirm = null;
// Whose password the reset form is for.
let resetting = null;
// Whose name the rename form is for.
let renaming = null;
// Whose account the delete form is for.
let deleting = null;
// The last list of people, for the delete form's choices.
let people = { people: [], you: null };

function showError(message) {
  $("error").textContent = message;
  $("error").hidden = !message;
}

// Says what a change did, above the list of people.
function showDone(message) {
  $("done").textContent = message;
  $("done").hidden = !message;
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

// Such as "October 30, 2026 at 3:04 PM", in the reader's time zone.
function when(iso) {
  return new Date(iso).toLocaleString(undefined, { dateStyle: "long", timeStyle: "short" });
}

function show(id) {
  for (const section of ["sign-in", "confirm", "reset", "rename", "delete", "link", "manage"]) {
    $(section).hidden = section !== id;
  }
}

function describe(person) {
  const link = person.password_link;
  if (person.state === "turned_off") {
    return "Turned off. They can't sign in, and their files are kept.";
  }
  if (person.state === "waiting") {
    if (!link) return "Waiting to set a password. Their link expired, so make a new one.";
    const start = link.purpose === "reset" ? "Password reset." : "Hasn't set a password yet.";
    return `${start} Their link works until ${when(link.expires_at)}.`;
  }
  return "";
}

function button(text, action) {
  const element = document.createElement("button");
  element.type = "button";
  element.className = "secondary";
  element.textContent = text;
  element.addEventListener("click", () => action(element));
  return element;
}

function render(data) {
  people = data;
  const list = $("people");
  list.replaceChildren();
  for (const person of data.people) {
    const item = document.createElement("li");
    const name = document.createElement("strong");
    name.textContent = person.display_name;
    const details = document.createElement("p");
    details.className = "hint";
    details.textContent = `${person.username} · ${ROLES[person.role] || person.role}`;
    item.append(name, details);
    const state = describe(person);
    if (state) {
      const text = document.createElement("p");
      text.textContent = state;
      item.append(text);
    }
    if (person.can_manage) {
      const buttons = [];
      if (person.state === "waiting") {
        buttons.push(button("Make a new link", (b) => makeLink(person, b)));
      } else if (person.state === "active") {
        buttons.push(button("Reset password", () => resetPassword(person)));
      }
      if (data.can_add_admins && person.state !== "turned_off") {
        const role = person.role === "admin" ? "user" : "admin";
        const text = role === "admin" ? "Make admin" : "Make standard account";
        buttons.push(button(text, (b) => setRole(person, role, b)));
      }
      buttons.push(button("Rename", () => renamePerson(person)));
      if (person.state === "turned_off") {
        buttons.push(button("Turn on", (b) => act(b, "POST", `/api/people/${person.id}/turn-on`)));
      } else {
        buttons.push(button("Turn off", (b) => turnOff(person, b)));
      }
      buttons.push(button("Delete", () => deletePerson(person)));
      for (const [index, element] of buttons.entries()) {
        if (index > 0) item.append(" ");
        item.append(element);
      }
    }
    list.append(item);
  }
  $("role-field").hidden = !data.can_add_admins;
  if (!data.can_add_admins) $("role").value = "user";
}

async function load() {
  try {
    const { status, data } = await api("GET", "/api/people");
    if (status === 401) {
      show("sign-in");
      return;
    }
    if (status !== 200) throw new Error(data.error || PROBLEM);
    render(data);
    const busy = ["link", "confirm", "reset", "rename", "delete"].some((id) => !$(id).hidden);
    if (!busy) show("manage");
  } catch (error) {
    showError(error.message);
  }
}

// Sends a change. If the sign-in isn't recent, asks for the password, then
// sends it again. Returns the data, or null while waiting for the password.
async function send(method, path, body) {
  const { status, data } = await api(method, path, body);
  if (status === 403 && data.error?.startsWith("For your security")) {
    afterConfirm = async () => {
      const retried = await send(method, path, body);
      if (retried) await afterChange(retried);
    };
    show("confirm");
    $("confirm-password").focus();
    return null;
  }
  if (status >= 300) throw new Error(data.error || PROBLEM);
  return data;
}

// What happens after a change goes through: show a new link, or reload.
let afterChange = async () => {
  show("manage");
  await load();
};

function showLink(data, name, purpose, deletedAppPasswords = false) {
  const address = `${location.origin}/welcome#${data.token}`;
  $("link-address").textContent = address;
  if (purpose === "reset") {
    $("link-title").textContent = `Reset link for ${name}`;
    const devices = deletedAppPasswords
      ? "Their app passwords are deleted, so each of their devices needs a new one."
      : "Their devices that use app passwords keep working.";
    $("link-help").textContent =
      `Send this link to ${name}, so they can choose a new password. They're signed out everywhere until they do. ${devices}`;
  } else {
    $("link-title").textContent = `Link for ${name}`;
    $("link-help").textContent =
      `Send this link to ${name}, so they can choose their password and sign in.`;
  }
  $("link-expires").textContent =
    `It works once, until ${when(data.expires_at)}. Anyone with the link can set the password, so send it only to ${name}, such as in a text message.`;
  show("link");
}

async function act(element, method, path, body, onDone) {
  showError("");
  showDone("");
  element.disabled = true;
  afterChange = onDone || (async () => {
    show("manage");
    await load();
  });
  try {
    const data = await send(method, path, body);
    if (data) await afterChange(data);
  } catch (error) {
    showError(error.message);
  } finally {
    element.disabled = false;
  }
}

function makeLink(person, element, body = {}) {
  return act(element, "POST", `/api/people/${person.id}/password-link`, body, async (data) =>
    showLink(data, person.display_name, data.purpose, body.delete_app_passwords),
  );
}

function resetPassword(person) {
  showError("");
  resetting = person;
  $("reset-title").textContent = `Reset ${person.display_name}'s password`;
  $("reset").querySelector('input[value="forgot"]').checked = true;
  show("reset");
}

function setRole(person, role, element) {
  const sure =
    role === "user" ||
    confirm(
      `Make ${person.display_name} an admin? Admins can add people, reset their passwords, check the server's health, and manage backups.`,
    );
  if (sure) return act(element, "PUT", `/api/people/${person.id}/role`, { role });
}

function turnOff(person, element) {
  const sure = confirm(
    `Turn off ${person.display_name}'s account? They'll be signed out everywhere, including File Explorer and other apps, and can't sign in until you turn it back on. Their files are kept.`,
  );
  if (sure) return act(element, "POST", `/api/people/${person.id}/turn-off`);
}

function renamePerson(person) {
  showError("");
  showDone("");
  renaming = person;
  $("rename-title").textContent = `Rename ${person.display_name}`;
  $("rename-display-name").value = person.display_name;
  $("rename-username").value = person.username;
  show("rename");
  $("rename-display-name").focus();
}

function givingFiles() {
  return $("delete").querySelector('input[value="give"]').checked;
}

function deletePerson(person) {
  showError("");
  showDone("");
  deleting = person;
  $("delete-title").textContent = `Delete ${person.display_name}'s account`;
  $("delete-username-label").textContent = `To confirm, type their username: ${person.username}`;
  $("delete-username").value = "";
  const choices = $("delete-give-to");
  choices.replaceChildren();
  for (const other of people.people) {
    if (other.id === person.id) continue;
    const option = document.createElement("option");
    option.value = other.id;
    option.textContent = other.id === people.you ? `${other.display_name} (you)` : other.display_name;
    option.selected = other.id === people.you;
    choices.append(option);
  }
  $("delete").querySelector('input[value="give"]').checked = true;
  $("delete-give-to").disabled = false;
  show("delete");
}

// What a deletion did, once it's done.
function deleted(person, data, givenTo) {
  if (data.folder) {
    return `${person.display_name}'s account is deleted. Their files are in ${givenTo}'s Drive, in the folder "${data.folder}".`;
  }
  if (givenTo) {
    return `${person.display_name}'s account is deleted. They didn't have any files.`;
  }
  return `${person.display_name}'s account and files are deleted.`;
}

function onSubmit(id, action) {
  $(id).addEventListener("submit", async (event) => {
    event.preventDefault();
    showError("");
    const element = event.submitter;
    element.disabled = true;
    try {
      await action();
    } catch (error) {
      showError(error.message);
    } finally {
      element.disabled = false;
    }
  });
}

onSubmit("sign-in", async () => {
  const { status, data } = await api("POST", "/api/session", {
    username: $("username").value,
    password: $("password").value,
  });
  if (status !== 200) throw new Error(data.error || PROBLEM);
  $("password").value = "";
  await load();
});

onSubmit("confirm", async () => {
  const { status, data } = await api("POST", "/api/session/confirm", {
    password: $("confirm-password").value,
  });
  if (status !== 204) throw new Error(data.error || PROBLEM);
  $("confirm-password").value = "";
  show("manage");
  const next = afterConfirm;
  afterConfirm = null;
  if (next) await next();
});

$("reset").addEventListener("submit", async (event) => {
  event.preventDefault();
  const reason = $("reset").querySelector('input[name="reason"]:checked').value;
  await makeLink(resetting, event.submitter, {
    delete_app_passwords: reason === "exposed",
  });
});

$("rename").addEventListener("submit", async (event) => {
  event.preventDefault();
  await act(event.submitter, "PUT", `/api/people/${renaming.id}/name`, {
    display_name: $("rename-display-name").value,
    username: $("rename-username").value,
  });
});

$("rename-cancel").addEventListener("click", () => {
  renaming = null;
  show("manage");
});

// The person list only matters while giving the files to someone.
$("delete").addEventListener("change", () => {
  $("delete-give-to").disabled = !givingFiles();
});

$("delete").addEventListener("submit", async (event) => {
  event.preventDefault();
  const person = deleting;
  const give = givingFiles();
  const to = $("delete-give-to").value;
  const givenTo = give ? people.people.find((other) => other.id === to)?.display_name : null;
  await act(
    event.submitter,
    "DELETE",
    `/api/people/${person.id}`,
    {
      username: $("delete-username").value,
      files: give ? { give_to: to } : "delete",
    },
    async (data) => {
      deleting = null;
      show("manage");
      await load();
      showDone(deleted(person, data, givenTo));
    },
  );
});

$("delete-cancel").addEventListener("click", () => {
  deleting = null;
  show("manage");
});

$("reset-cancel").addEventListener("click", () => {
  resetting = null;
  show("manage");
});

$("confirm-cancel").addEventListener("click", () => {
  afterConfirm = null;
  $("confirm-password").value = "";
  show("manage");
});

onSubmit("add", async () => {
  const name = $("display-name").value.trim();
  afterChange = async (data) => {
    $("display-name").value = "";
    $("new-username").value = "";
    $("role").value = "user";
    await load();
    showLink(data, name, "new_account");
  };
  const data = await send("POST", "/api/people", {
    display_name: name,
    username: $("new-username").value,
    role: $("role").value,
  });
  if (data) await afterChange(data);
});

$("copy").addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText($("link-address").textContent);
    $("copy").textContent = "Copied";
  } catch {
    // Clipboard access needs HTTPS. Select the link so it's one copy away.
    const range = document.createRange();
    range.selectNodeContents($("link-address"));
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    showError("Accordion couldn't copy the link, so it's selected for you to copy.");
  }
});

$("link-done").addEventListener("click", async () => {
  $("link-address").textContent = "";
  $("copy").textContent = "Copy link";
  showError("");
  show("manage");
  await load();
});

load();
