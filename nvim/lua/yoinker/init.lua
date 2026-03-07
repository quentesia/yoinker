local M = {}

-- Default config
M.config = {
  socket_path = nil, -- auto-detect from XDG_RUNTIME_DIR
  keymap_prefix = "<leader>y",
}

--- Get the socket path, respecting XDG_RUNTIME_DIR
local function get_socket_path()
  if M.config.socket_path then
    return M.config.socket_path
  end
  local runtime_dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
  return runtime_dir .. "/yoinker.sock"
end

--- Send a request to the yoinker daemon and return the response
---@param request table
---@return table|nil
local function send_request(request)
  local socket_path = get_socket_path()
  local json = vim.fn.json_encode(request)

  -- Use socat to talk to the Unix socket
  local result = vim.fn.system(
    string.format("echo '%s' | socat - UNIX-CONNECT:%s", json, socket_path)
  )

  if vim.v.shell_error ~= 0 then
    vim.notify("yoinker: daemon not running", vim.log.levels.ERROR)
    return nil
  end

  return vim.fn.json_decode(result)
end

--- Store the current visual selection in yoinker
function M.store_selection(pin)
  pin = pin or false
  -- Get visual selection
  local start_pos = vim.fn.getpos("'<")
  local end_pos = vim.fn.getpos("'>")
  local lines = vim.fn.getregion(start_pos, end_pos, { type = vim.fn.visualmode() })
  local text = table.concat(lines, "\n")

  if text == "" then
    vim.notify("yoinker: empty selection", vim.log.levels.WARN)
    return
  end

  send_request({ Store = { content = text, pin = pin } })
  vim.notify(pin and "yoinker: pinned selection" or "yoinker: stored selection")
end

--- Paste from yoinker history at index
function M.paste(index)
  local resp = send_request({ Get = { index = index } })
  if resp and resp.Entry then
    local content = resp.Entry.content
    if content.type == "Text" then
      vim.api.nvim_put(vim.split(content.data, "\n"), "", true, true)
    end
  end
end

--- Open yoinker list in a terminal
function M.list()
  vim.cmd("terminal yoinker list")
end

--- Setup function for lazy.nvim / packer
function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", M.config, opts or {})

  local prefix = M.config.keymap_prefix

  -- Keymaps
  vim.keymap.set("v", prefix .. "y", function() M.store_selection(false) end,
    { desc = "Yoinker: store selection" })
  vim.keymap.set("v", prefix .. "p", function() M.store_selection(true) end,
    { desc = "Yoinker: pin selection" })
  vim.keymap.set("n", prefix .. "l", function() M.list() end,
    { desc = "Yoinker: list history" })
  vim.keymap.set("n", prefix .. "1", function() M.paste(0) end,
    { desc = "Yoinker: paste most recent" })
end

return M
