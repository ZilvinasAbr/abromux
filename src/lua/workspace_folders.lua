-- Workspace folders: a workspace with a folder of its own, a scoped place
-- for an agent to work in, and its cleanup. Written in Lua on abromux's
-- API; turn it on in init.lua:
--
--     require("abromux.workspace_folders").setup{}
--
-- That adds two palette actions and a command:
--
-- * New Workspace Folder… (⇧⌘I): asks for a name (Enter alone takes a
--   made-up one), makes `<root>/<name>/` with `.abromux/workspace.json`, an
--   `AGENTS.md` and a `CLAUDE.md` importing it, and opens a workspace there.
--   Its terminals get `$ABROMUX_WORKSPACE_DIR` and `$COMPOSE_PROJECT_NAME`.
-- * Clean Up and Delete Workspace Folder… (⇧⌘X): moves the workspace to the
--   Cleanup group and starts an agent on `.abromux/cleanup.md`, which ends
--   with `abromux folder remove`.
-- * `abromux folder [path|root|add|list|all|repos|resources|remove]`, from
--   any shell (abromux runs this file's `folder` action for it).
--
-- setup{} takes:
--   root          where folders go (default: $ABROMUX_WORKSPACES_DIR, else
--                 config.json's app.workspaceFoldersDirectory, else
--                 ~/.abromux/ws, per channel)
--   agent         the cleanup agent's command (default: config.json's
--                 automation.workspaceCleanupAgent, else "claude")
--   key           New Workspace Folder's shortcut (default "cmd-shift-i")
--   cleanup_key   Clean Up's shortcut (default "cmd-shift-x")
--   agents_md     the AGENTS.md text, to write your own
--   cleanup_md    the cleanup instructions, likewise
--
-- The functions under M (create, add, remove, ...) work without the app,
-- on files and git, so other scripts can build on them.

local A = abromux
local M = {}

local META = ".abromux/workspace.json"
M.LEDGER = ".abromux/resources.md"
M.CLEANUP = ".abromux/cleanup.md"
M.ASK = "Read .abromux/cleanup.md and follow it."

M.AGENTS_MD = [==[
# AbroMux workspace

This folder is a scoped workspace made by AbroMux. Do your work here, not in
the projects' own folders elsewhere on this machine.

To work on a project, check it out into this folder first:

    abromux folder add <path-to-project> [--branch <name>]

For a git repo that makes a worktree here on a new branch (`ws/<folder>` by
default), so the original checkout and its branch stay untouched. A folder
that isn't a git repo is copied. `abromux folder list` shows what is checked
out; `abromux folder repos` lists git repos it knows of on this machine.

Commit in the worktrees as usual: the commits land in the original repo.

## Tickets and merge requests

Pin the Jira tickets and GitLab merge requests this work is for, so they
show under the workspace in the sidebar. Each opens in a pinned tab, hidden
until the user shows it:

    abromux link add PAY-1423
    abromux link add https://gitlab.example.com/group/project/-/merge_requests/12

Any tab can be pinned, a terminal running a long task too:
`abromux tab pin [--surface <ref>]`. Closing a pinned tab only hides it.
`abromux link` lists the pinned tabs; `abromux link remove <key>` (or
`abromux tab unpin --surface <ref>`) unpins one that's no longer part of the
work.

## Resources outside this folder

When the workspace is done, everything made for it is removed, so keep track
of what you make outside this folder:

- `docker compose` stacks are named after this workspace already
  (`$COMPOSE_PROJECT_NAME`); keep it, don't pass `-p` or set `name:`.
- Give anything you `docker run` / `docker volume create` / `docker network
  create` the label `--label abromux.workspace=$COMPOSE_PROJECT_NAME`.
- Write anything else down in `.abromux/resources.md`, one line each: what
  it is and the command that removes it (a database or user on a shared
  server, a cloud bucket, a `brew services` entry, a launchd job, a
  long-running process, a port forward…).

`abromux folder resources` shows what has been recorded and found.
]==]

M.CLEANUP_MD = [==[
# Clean up this workspace

This workspace is finished. Remove everything that was made for it, then
delete it. You are running inside its folder; the last step ends this session.

Workspaces are scratch folders for one task: nothing in them is meant to
last. Don't ask the user to confirm; just do it. Stop and ask only when
something would really be lost (step 2).

1. Find what it made:
   - `abromux folder resources`: what agents recorded in
     `.abromux/resources.md`, and the Docker containers, volumes and
     networks labelled for this workspace or in its compose project.
   - `abromux folder list`: the projects checked out here (git worktrees, or
     copies of folders that aren't repos).
   - Anything else made or started for this workspace that wasn't recorded:
     look at compose files, scripts, `.env` files and READMEs in the
     checkouts for things they create (databases, buckets, queues, k8s
     namespaces, images…) and check whether those exist now; processes still
     running from this folder (`lsof -a -d cwd +D "$PWD"`, and
     `ps -eo pid,command | grep -F "$PWD"`) and the ports they listen on.
2. Check what would be lost: in each worktree, uncommitted changes (ignore
   build output and other generated files) and commits not pushed or merged
   anywhere (squash merges count as merged). Copies of folders that aren't
   repos are deleted outright, so check whether one was changed. If any of
   that turns up, this is the one time to stop: show the user exactly what
   would be lost and wait for them to say go ahead (then use `--force` in
   step 5) or stop. Otherwise don't ask anything.
3. Only remove what was made for this workspace. Anything shared with other
   projects, or that was there before, stays (base images pulled from a
   registry, a database server other things use); keep it without asking
   and mention it at the end.
4. Remove it: stop the processes, `docker compose -p "$COMPOSE_PROJECT_NAME"
   down -v --rmi local` for the compose project, `docker rm -f`, `docker
   volume rm` and `docker network rm` for labelled leftovers, the commands
   in `.abromux/resources.md`, and so on. Check that each one is gone.
5. Last, run `abromux folder remove`. It removes the worktrees (keeping their
   branches unless they're merged), deletes this folder and closes the
   workspace, which ends this session, so first print a short list of what
   you removed and kept. If it refuses, tell the user why and stop; add
   `--force` only if they agreed in step 2 to lose that work.
]==]

local opts = {}

-- ---- files and commands ------------------------------------------------

local function fail(msg)
  error(msg, 0)
end

--- Run argv; its stdout without the trailing newline, or nil and why.
local function run(argv, o)
  local r, err = A.exec(argv, o or {})
  if not r then
    return nil, err
  end
  if r.code ~= 0 then
    local e = ((r.stderr ~= "" and r.stderr) or r.stdout or ""):gsub("%s+$", "")
    return nil, e ~= "" and e or (argv[1] .. " exited with " .. tostring(r.code or r.signal))
  end
  return (r.stdout:gsub("%s+$", ""))
end

local function git(dir, ...)
  return run({ "git", "-C", dir, ... })
end

local function exists(path)
  local ok, _, code = os.rename(path, path)
  return ok ~= nil or code == 13
end

local function is_dir(path)
  return run({ "test", "-d", path }) ~= nil
end

local function read(path)
  local f = io.open(path, "r")
  if not f then
    return nil
  end
  local text = f:read("a")
  f:close()
  return text
end

local function write(path, text)
  local f, err = io.open(path, "w")
  if not f then
    fail(err)
  end
  f:write(text)
  f:close()
end

local function mkdir(path)
  local _, err = run({ "mkdir", "-p", path })
  if err then
    fail(err)
  end
end

local function basename(path)
  return (path:gsub("/+$", ""):match("([^/]+)$")) or path
end

local function home()
  return os.getenv("HOME") or ""
end

local function expand(path)
  if path == "~" then
    return home()
  end
  if path:sub(1, 2) == "~/" then
    return home() .. path:sub(2)
  end
  return path
end

local function tildify(path)
  local h = home()
  if h ~= "" and path:sub(1, #h + 1) == h .. "/" then
    return "~" .. path:sub(#h + 1)
  end
  return path
end

--- The real path of folder `path` (relative to this process's folder).
local function realdir(path)
  return run({ "sh", "-c", 'cd -- "$1" && pwd -P', "sh", expand(path) })
end

local function config()
  local text = read(A.config_path)
  local c = text and A.json.decode(text)
  return type(c) == "table" and c or {}
end

-- ---- workspace folders ---------------------------------------------------

function M.read_meta(dir)
  local text = read(dir .. "/" .. META)
  local meta = text and A.json.decode(text)
  if type(meta) ~= "table" then
    return nil
  end
  meta.checkouts = meta.checkouts or A.json.array()
  return meta
end

local function write_meta(dir, meta)
  mkdir(dir .. "/.abromux")
  write(dir .. "/" .. META, A.json.encode(meta, { indent = true }) .. "\n")
end

--- A folder name for `name`: lowercase letters, digits and dashes.
function M.slug(name)
  local s = name:lower():gsub("[^%w\128-\255]+", "-"):gsub("^%-+", ""):gsub("%-+$", "")
  return s ~= "" and s or "workspace"
end

--- `base/name`, or `base/name-2`, `-3`… if that's taken.
local function unique(base, name)
  local p = base .. "/" .. name
  local n = 1
  while exists(p) do
    n = n + 1
    p = ("%s/%s-%d"):format(base, name, n)
  end
  return p
end

local ADJ = {
  "amber", "brave", "calm", "crisp", "dusty", "eager", "fuzzy", "gentle", "hidden", "jolly",
  "keen", "lively", "mellow", "nimble", "quiet", "rapid", "rusty", "silent", "sunny",
  "swift", "tidy", "vivid", "witty", "zesty",
}
local NOUN = {
  "anchor", "badger", "beacon", "canyon", "comet", "falcon", "garden", "harbor", "island",
  "lantern", "meadow", "otter", "pebble", "quarry", "river", "summit", "thicket", "tundra",
  "valley", "willow", "orchard", "glacier", "ember", "forge",
}

--- A name like `quiet-amber-falcon`, for a workspace not named yet.
function M.random_name()
  local r = math.random
  return ("%s-%s-%s"):format(ADJ[r(#ADJ)], ADJ[r(#ADJ)], NOUN[r(#NOUN)])
end

--- Where workspace folders go.
function M.root()
  local set = os.getenv("ABROMUX_WORKSPACES_DIR")
  if not set or set:match("^%s*$") then
    set = opts.root
  end
  if not set or set:match("^%s*$") then
    set = (config().app or {}).workspaceFoldersDirectory
  end
  if type(set) ~= "string" or set:match("^%s*$") then
    return A.workspaces_dir
  end
  return expand(set:match("^%s*(.-)%s*$"))
end

--- Make a workspace folder for `name` under `base`; its path.
function M.create(base, name)
  mkdir(base)
  local dir = unique(base, M.slug(name))
  mkdir(dir)
  write_meta(dir, { name = name, created = os.time(), checkouts = A.json.array() })
  write(dir .. "/AGENTS.md", opts.agents_md or M.AGENTS_MD)
  write(dir .. "/CLAUDE.md", "@AGENTS.md\n")
  return dir
end

--- The workspace folder `path` is in: the nearest one holding META.
function M.find(path)
  local p = path:gsub("/+$", "")
  while p ~= "" do
    if exists(p .. "/" .. META) then
      return p
    end
    p = p:match("^(.*)/[^/]*$") or ""
  end
  return nil
end

--- The workspace folder this shell is in.
function M.current()
  local d = os.getenv("ABROMUX_WORKSPACE_DIR")
  if d and d ~= "" then
    return d
  end
  local found = M.find(os.getenv("PWD") or run({ "/bin/pwd" }) or "")
  if not found then
    fail("not in a workspace folder (make one with New Workspace Folder…)")
  end
  return found
end

--- Check `source` out into workspace folder `ws`: a worktree on `branch`
--- (default `ws/<folder>`) for a git repo, a copy for anything else.
function M.add(ws, source, branch)
  local meta = M.read_meta(ws) or fail(ws .. ": not a workspace folder")
  local src, err = realdir(source)
  if not src then
    fail(source .. ": " .. (err or "not a folder"))
  end
  if src == ws or src:sub(1, #ws + 1) == ws .. "/" then
    fail(src .. " is already in this workspace")
  end
  local top = git(src, "rev-parse", "--show-toplevel")
  local dest = unique(ws, basename(top or src))
  local c = { path = basename(dest), source = top or src }
  if top then
    branch = branch or ("ws/" .. basename(ws))
    local had = git(top, "rev-parse", "--verify", "--quiet", "refs/heads/" .. branch) ~= nil
    local _, e
    if had then
      _, e = git(top, "worktree", "add", dest, branch)
    else
      _, e = git(top, "worktree", "add", "-b", branch, dest)
    end
    if e then
      fail(e)
    end
    c.branch = branch
    c.new_branch = (not had) or nil
  else
    local _, e = run({ "/bin/cp", "-Rp", src, dest })
    if e then
      fail(e)
    end
  end
  table.insert(meta.checkouts, c)
  write_meta(ws, meta)
  return c
end

--- The name Docker resources of `ws` go by: its folder's name.
function M.tag(ws)
  return M.slug(basename(ws))
end

--- Docker containers, volumes and networks of `ws`, a line each; nil and
--- why when Docker isn't there or isn't running.
function M.docker_resources(ws)
  local tag = M.tag(ws)
  local kinds = {
    { "container", { "ps", "-a" }, "{{.Names}}  {{.Image}}  {{.Status}}" },
    { "volume", { "volume", "ls" }, "{{.Name}}" },
    { "network", { "network", "ls" }, "{{.Name}}" },
  }
  local out, seen = {}, {}
  for _, k in ipairs(kinds) do
    for _, label in ipairs({ "abromux.workspace", "com.docker.compose.project" }) do
      local argv = { "docker", table.unpack(k[2]) }
      for _, a in ipairs({ "--filter", ("label=%s=%s"):format(label, tag), "--format", k[3] }) do
        argv[#argv + 1] = a
      end
      local text, err = run(argv)
      if not text then
        return nil, err
      end
      for line in text:gmatch("[^\n]+") do
        line = k[1] .. " " .. line:match("^%s*(.-)%s*$")
        if not seen[line] then
          seen[line] = true
          out[#out + 1] = line
        end
      end
    end
  end
  return out
end

--- Worktrees in `ws` with uncommitted or untracked changes, a line each.
function M.blockers(ws)
  local out = {}
  for _, c in ipairs((M.read_meta(ws) or {}).checkouts or {}) do
    if c.branch then
      local status = git(ws .. "/" .. c.path, "status", "--porcelain")
      local n = status and select(2, status:gsub("[^\n]+", "")) or 0
      if n > 0 then
        out[#out + 1] = ("%s: %d uncommitted or untracked file(s)"):format(c.path, n)
      end
    end
  end
  return out
end

--- Delete workspace folder `ws`: each worktree out of its repo (and the
--- branch `add` made for it, if merged), then the folder. Without `force`
--- it refuses while a worktree has changes. What it did, a line each.
function M.remove(ws, force)
  local meta = M.read_meta(ws) or fail(ws .. ": not a workspace folder")
  if ws == "" or ws == "/" or ws == home() then
    fail(ws .. ": won't delete that")
  end
  if not force then
    local b = M.blockers(ws)
    if #b > 0 then
      fail("work would be lost (commit it, or --force to drop it):\n  " .. table.concat(b, "\n  "))
    end
  end
  local done = {}
  for _, c in ipairs(meta.checkouts) do
    local path = ws .. "/" .. c.path
    if not c.branch then
      done[#done + 1] = ("%s: deleted (a copy of %s)"):format(c.path, c.source)
    elseif not is_dir(c.source) then
      done[#done + 1] = ("%s: %s is gone, deleted its files"):format(c.path, c.source)
    else
      if exists(path) then
        local _, e
        if force then
          _, e = git(c.source, "worktree", "remove", "--force", path)
        else
          _, e = git(c.source, "worktree", "remove", path)
        end
        if e then
          fail(c.path .. ": " .. e)
        end
      else
        git(c.source, "worktree", "prune")
      end
      local note
      if not c.new_branch then
        note = ("kept branch %s (it was there before)"):format(c.branch)
      elseif git(c.source, "branch", "-d", c.branch) then
        note = ("deleted branch %s (merged)"):format(c.branch)
      else
        note = ("kept branch %s (not merged)"):format(c.branch)
      end
      done[#done + 1] = ("%s: removed the worktree from %s, %s"):format(c.path, c.source, note)
    end
  end
  local _, e = run({ "rm", "-rf", ws })
  if e then
    fail(ws .. ": " .. e)
  end
  done[#done + 1] = "deleted " .. ws
  return done
end

--- Every workspace folder under `root`, oldest first: {dir, meta} each.
function M.list(root)
  local names = run({ "ls", "-1A", root }) or ""
  local out = {}
  for name in names:gmatch("[^\n]+") do
    local dir = root .. "/" .. name
    local meta = M.read_meta(dir)
    if meta then
      out[#out + 1] = { dir = dir, meta = meta }
    end
  end
  table.sort(out, function(a, b)
    local x, y = a.meta.created or 0, b.meta.created or 0
    if x ~= y then
      return x < y
    end
    return a.dir < b.dir
  end)
  return out
end

--- Git repos right under the usual project folders.
function M.known_repos()
  local out = {}
  for _, r in ipairs({ "repos", "src", "code", "projects", "dev", "Developer" }) do
    local base = home() .. "/" .. r
    for name in (run({ "ls", "-1", base }) or ""):gmatch("[^\n]+") do
      if exists(base .. "/" .. name .. "/.git") then
        out[#out + 1] = base .. "/" .. name
      end
    end
  end
  return out
end

-- ---- with the app ----------------------------------------------------------

--- The open workspace whose folder is `dir`, if the app is running.
local function workspace_of(dir)
  local list = A.workspace.list{}
  for _, w in ipairs(list or {}) do
    if w.home and (w.home == dir or realdir(w.home) == dir) then
      return w
    end
  end
end

local function new_folder(ctx, args)
  local root = M.root()
  local name, named = args.name, args.name ~= nil
  if not name then
    local suggested = M.random_name()
    local typed = A.ui.input{
      title = "New workspace folder in " .. tildify(root),
      placeholder = suggested,
      confirm = "Create",
    }
    if typed == nil then
      return
    end
    typed = typed:gsub("%s+", " "):match("^%s*(.-)%s*$")
    named = typed ~= ""
    name = named and typed or suggested
  end
  local dir = M.create(root, name)
  local ws, err = A.workspace.create{
    name = name,
    folder = dir,
    -- A made-up name is a stand-in: its agent names it (auto-naming).
    provisional = not named,
    -- Compose stacks started there are the workspace's, for cleanup.
    env = { COMPOSE_PROJECT_NAME = M.tag(dir) },
  }
  if not ws then
    fail(err)
  end
  return { workspace = ws.ref, folder = dir }
end

local function clean_up(ctx, args)
  local ref = args.workspace or ctx.workspace
  local w
  for _, x in ipairs(A.workspace.list{} or {}) do
    if x.ref == ref or (ref == nil and x.selected) then
      w = x
    end
  end
  if not w then
    fail("no workspace " .. tostring(ref))
  end
  if not w.home or not M.read_meta(w.home) then
    fail(w.name .. " isn't a workspace folder: only one made with New Workspace Folder… has a folder to clean up")
  end
  mkdir(w.home .. "/.abromux")
  write(w.home .. "/" .. M.CLEANUP, opts.cleanup_md or M.CLEANUP_MD)
  local agent = opts.agent or (config().automation or {}).workspaceCleanupAgent
  if type(agent) ~= "string" or agent:match("^%s*$") then
    agent = "claude"
  end
  A.workspace.action{ workspace = w.ref, action = "move-to-cleanup" }
  A.workspace.select{ workspace = w.ref }
  local params = {
    workspace = w.ref,
    cwd = w.home,
    command = agent:match("^%s*(.-)%s*$") .. " '" .. M.ASK .. "'",
  }
  if w.ref ~= ctx.workspace then
    -- A tab of that workspace, not beside whatever ran this.
    params.caller = false
  end
  local tab, err = A.surface.create(params)
  if not tab then
    fail(err)
  end
  return { workspace = w.ref, folder = w.home }
end

local function out(line)
  io.write(line, "\n")
end

--- `abromux folder <sub> ...`.
local function folder_cmd(ctx, args)
  local sub = args[1] or "path"
  if sub == "path" then
    out(M.current())
  elseif sub == "root" then
    out(M.root())
  elseif sub == "add" then
    local src = args[2] or fail("folder add: missing <path>")
    local ws = M.current()
    local c = M.add(ws, src, args.branch)
    if c.branch then
      out(("OK %s/%s (worktree of %s on %s)"):format(ws, c.path, c.source, c.branch))
    else
      out(("OK %s/%s (copy of %s)"):format(ws, c.path, c.source))
    end
  elseif sub == "list" then
    for _, c in ipairs((M.read_meta(M.current()) or { checkouts = {} }).checkouts) do
      out(("%s  %s  from %s"):format(c.path, c.branch and ("worktree " .. c.branch) or "copy", c.source))
    end
  elseif sub == "resources" then
    local ws = M.current()
    local text = read(ws .. "/" .. M.LEDGER)
    if text and text:match("%S") then
      out("Recorded in " .. M.LEDGER .. ":")
      out((text:gsub("%s+$", "")))
    else
      out("Nothing recorded in " .. M.LEDGER .. ".")
    end
    local tag = M.tag(ws)
    local found, err = M.docker_resources(ws)
    if not found then
      out("Docker not checked: " .. err)
    elseif #found == 0 then
      out(("No Docker resources labelled abromux.workspace=%s or in compose project %s."):format(tag, tag))
    else
      out(("Docker (labelled abromux.workspace=%s or in compose project %s):"):format(tag, tag))
      for _, l in ipairs(found) do
        out("  " .. l)
      end
    end
  elseif sub == "remove" then
    local ws
    if args[2] then
      ws = realdir(args[2]) or fail(args[2] .. ": no such folder")
      if not M.read_meta(ws) then
        fail(ws .. ": not a workspace folder")
      end
    else
      ws = M.current()
    end
    local force = args.force == true
    if not force then
      local problems = M.blockers(ws)
      local left, err = M.docker_resources(ws)
      if left and #left > 0 then
        problems[#problems + 1] = ("Docker resources are still there: %s (`abromux folder resources`)"):format(
          table.concat(left, ", ")
        )
      elseif not left then
        io.stderr:write("abromux: Docker not checked for leftovers: ", err, "\n")
      end
      if #problems > 0 then
        fail(("not removing %s; clean these up first, or --force:\n  %s"):format(ws, table.concat(problems, "\n  ")))
      end
    end
    -- Found first: the app can't tell once the folder is gone.
    local w = workspace_of(ws)
    for _, line in ipairs(M.remove(ws, force)) do
      out(line)
    end
    if w then
      -- Last: this shell (and an agent in it) may be one of its terminals.
      out("closing " .. w.ref)
      io.stdout:flush()
      A.workspace.close{ workspace = w.ref }
    end
  elseif sub == "all" then
    local open
    local list = A.workspace.list{}
    if list then
      open = {}
      for _, w in ipairs(list) do
        if w.home then
          open[w.home] = true
        end
      end
    end
    for _, f in ipairs(M.list(M.root())) do
      local mark = open == nil and "" or (open[f.dir] and "  open" or "  closed")
      if not (args.closed and mark ~= "  closed") then
        out(("%s  %s  (%d checked out)%s"):format(f.dir, f.meta.name or "", #f.meta.checkouts, mark))
      end
    end
  elseif sub == "repos" then
    for _, r in ipairs(M.known_repos()) do
      out(r)
    end
  else
    fail(("folder: unknown subcommand %q (path, root, add, list, all, repos, resources, remove)"):format(sub))
  end
  return true
end

function M.setup(o)
  opts = o or {}
  A.action{
    name = "new_workspace_folder",
    title = "New Workspace Folder…",
    description = "A workspace with a folder of its own, for an agent to work in",
    keywords = "folder agent scoped",
    key = opts.key == nil and "cmd-shift-i" or opts.key or nil,
    args = { name = "string?" },
    run = new_folder,
  }
  A.action{
    name = "clean_up_workspace_folder",
    title = "Clean Up and Delete Workspace Folder…",
    description = "An agent removes what the workspace made, then the folder",
    keywords = "delete remove folder",
    key = opts.cleanup_key == nil and "cmd-shift-x" or opts.cleanup_key or nil,
    args = { workspace = "string?" },
    run = clean_up,
  }
  A.action{
    name = "folder",
    title = "Workspace Folder (abromux folder …)",
    description = "abromux folder path|root|add|list|all|repos|resources|remove",
    -- A command for shells and agents; nothing to show in the palette.
    palette = false,
    run = folder_cmd,
  }
  return M
end

return M
