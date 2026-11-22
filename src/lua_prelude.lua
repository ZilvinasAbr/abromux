-- The part of the `abromux` table written in Lua (src/lua.rs runs it
-- before a script or init.lua). `abromux.call`, `_log`, `_timer`,
-- `_cancel`, `_active` and the values (`args`, `surface_id`, ...) are set
-- from Rust first.

local A = abromux
local call = A.call

-- `abromux.<namespace>.<method>{...}` calls the socket method
-- `<namespace>.<method>`, so every method `abromux rpc list` shows is there.
local function namespace(name, t)
  return setmetatable(t or {}, {
    __index = function(self, method)
      if type(method) ~= "string" then
        return nil
      end
      local f = function(params)
        return call(name .. "." .. method, params)
      end
      rawset(self, method, f)
      return f
    end,
  })
end

-- `abromux.log("text")` / `abromux.log("warn", "text")` prints a line;
-- `abromux.log.add{...}` is the sidebar's log.
A.log = namespace("log")
getmetatable(A.log).__call = function(_, level, message)
  return A._log(level, message)
end

-- Questions in the command palette (the app's `ui.prompt`).
local function prompt(kind, t)
  if type(t) ~= "table" then
    error("abromux.ui." .. kind .. ": expected a table", 3)
  end
  if type(t.title) ~= "string" then
    error("abromux.ui." .. kind .. ": title is required", 3)
  end
  local p = {}
  for k, v in pairs(t) do
    p[k] = v
  end
  p.kind = kind
  local r, err = call("ui.prompt", p)
  if r == nil then
    if err and err:find("not running", 1, true) then
      return nil, "no_client"
    end
    return nil, "failed", err
  end
  if r.answer == nil then
    return nil, r.reason or "cancelled"
  end
  return r.answer
end

A.ui = namespace("ui", {
  input = function(t)
    return prompt("input", t)
  end,
  pick = function(t)
    return prompt("pick", t)
  end,
})

for _, name in ipairs(A._namespaces) do
  if rawget(A, name) == nil then
    A[name] = namespace(name)
  end
end

-- Timers: a table with cancel() and active().
local function timer(id)
  return {
    cancel = function()
      A._cancel(id)
    end,
    active = function()
      return A._active(id)
    end,
  }
end

function A.every(seconds, fn)
  return timer(A._timer(seconds, fn, true))
end

function A.after(seconds, fn)
  return timer(A._timer(seconds, fn, false))
end

-- Actions: named functions for the palette, keys, the CLI and invoke.
local actions, order = {}, {}
A._actions, A._order = actions, order

local types = {
  string = true,
  number = true,
  integer = true,
  boolean = true,
  object = true,
  array = true,
  any = true,
}

function A.action(def)
  if type(def) ~= "table" then
    error("abromux.action: expected a table", 2)
  end
  local name = def.name
  if type(name) ~= "string" or not name:match("^[a-z0-9_]+$") then
    error("abromux.action: name must be lowercase letters, digits and underscores", 2)
  end
  if type(def.title) ~= "string" or def.title == "" then
    error("abromux.action " .. name .. ": title is required", 2)
  end
  if type(def.run) ~= "function" then
    error("abromux.action " .. name .. ": run must be a function", 2)
  end
  if def.args ~= nil then
    if type(def.args) ~= "table" then
      error("abromux.action " .. name .. ": args must be a table of types", 2)
    end
    for k, ty in pairs(def.args) do
      if type(k) ~= "string" or type(ty) ~= "string" or not types[(ty:gsub("%?$", ""))] then
        error(("abromux.action %s: argument %s has an unknown type %s"):format(name, tostring(k), tostring(ty)), 2)
      end
    end
  end
  if actions[name] == nil then
    order[#order + 1] = name
  end
  actions[name] = def
end

local function is_array(v)
  if type(v) ~= "table" then
    return false
  end
  local n = 0
  for _ in pairs(v) do
    n = n + 1
  end
  return n == #v
end

-- Check (and copy) `args` against an action's declared types. A number or
-- boolean given for a string becomes its text, so `name=123` works.
local function checked(name, spec, args)
  local out = {}
  for k, v in pairs(args) do
    out[k] = v
  end
  if spec == nil then
    return out
  end
  for k in pairs(out) do
    if spec[k] == nil then
      local names = {}
      for n, ty in pairs(spec) do
        names[#names + 1] = n .. (ty:sub(-1) == "?" and "?" or "")
      end
      table.sort(names)
      local takes = #names > 0 and table.concat(names, ", ") or "no arguments"
      return nil, ("no argument named %q; %s takes %s"):format(tostring(k), name, takes)
    end
  end
  for k, ty in pairs(spec) do
    local optional = ty:sub(-1) == "?"
    local base = optional and ty:sub(1, -2) or ty
    local v = out[k]
    if v == nil then
      if not optional then
        return nil, ("missing argument %q"):format(k)
      end
    elseif base == "string" and (type(v) == "number" or type(v) == "boolean") then
      out[k] = tostring(v)
    elseif base ~= "any" then
      local ok = (base == "string" and type(v) == "string")
        or (base == "number" and type(v) == "number")
        or (base == "integer" and math.type(v) == "integer")
        or (base == "boolean" and type(v) == "boolean")
        or (base == "array" and is_array(v))
        or (base == "object" and type(v) == "table")
      if not ok then
        return nil, ("argument %q should be %s, got %s"):format(k, base, type(v))
      end
    end
  end
  return out
end

-- Run action `name`; errors in it are raised. Returns its result, or nil
-- and why it didn't run.
function A._run_action(name, args, origin)
  local def = actions[name]
  if def == nil then
    return nil, "no action named " .. tostring(name)
  end
  local a, bad = checked(name, def.args, args or {})
  if a == nil then
    return nil, bad
  end
  local ctx = { workspace = A.workspace_id, surface = A.surface_id, origin = origin }
  return def.run(ctx, a)
end

function A.invoke(name, args)
  local ok, r, err = pcall(A._run_action, name, args, "script")
  if not ok then
    return nil, tostring(r)
  end
  return r, err
end
