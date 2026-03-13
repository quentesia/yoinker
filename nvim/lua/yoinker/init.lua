local M = {}

-- Default config
M.config = {
  socket_path = nil, -- auto-detect from XDG_RUNTIME_DIR
  keymap_prefix = "<leader>y",
  picker_width = 80,
  picker_height = 20,
}

--- Get the socket path, respecting XDG_RUNTIME_DIR
local function get_socket_path()
  if M.config.socket_path then
    return M.config.socket_path
  end
  local runtime_dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
  return runtime_dir .. "/yoinker.sock"
end

--- Send a request to the yoinker daemon via libuv Unix socket (no socat needed).
--- Calls callback(response_table) on success, callback(nil) on failure.
---@param request any JSON-encodable request
---@param callback fun(resp: table|nil)|nil
local function send_request(request, callback)
  local socket_path = get_socket_path()
  local json = vim.fn.json_encode(request)
  local pipe = vim.uv.new_pipe(false)

  pipe:connect(socket_path, function(err)
    if err then
      if pipe and not pipe:is_closing() then
        pipe:close()
      end
      vim.schedule(function()
        vim.notify("yoinker: daemon not running (" .. err .. ")", vim.log.levels.ERROR)
        if callback then callback(nil) end
      end)
      return
    end

    -- Write request with newline delimiter
    pipe:write(json .. "\n", function(write_err)
      if write_err then
        if pipe and not pipe:is_closing() then
          pipe:close()
        end
        vim.schedule(function()
          vim.notify("yoinker: write error", vim.log.levels.ERROR)
          if callback then callback(nil) end
        end)
        return
      end

      -- Shutdown write side to signal end of request
      pipe:shutdown(function()
        -- Read response
        local chunks = {}
        pipe:read_start(function(read_err, data)
          if read_err then
            if pipe and not pipe:is_closing() then
              pipe:close()
            end
            vim.schedule(function()
              if callback then callback(nil) end
            end)
            return
          end

          if data then
            table.insert(chunks, data)
          else
            -- EOF
            if pipe and not pipe:is_closing() then
              pipe:close()
            end
            local response_str = table.concat(chunks, "")
            vim.schedule(function()
              if callback then
                local ok, resp = pcall(vim.fn.json_decode, response_str)
                callback(ok and resp or nil)
              end
            end)
          end
        end)
      end)
    end)
  end)
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

  send_request({ Store = { content = text, pin = pin } }, function(resp)
    if resp then
      vim.notify(pin and "yoinker: pinned selection" or "yoinker: stored selection")
    end
  end)
end

--- Paste from yoinker history at index
function M.paste(index)
  send_request({ Get = { index = index } }, function(resp)
    if resp and resp.Entry then
      local content = resp.Entry.content
      if content.type == "Text" then
        vim.api.nvim_put(vim.split(content.text, "\n"), "", true, true)
      end
    end
  end)
end

--- Open a floating window picker for yoinker history
function M.list()
  send_request("List", function(resp)
    if not resp or not resp.Entries then
      vim.notify("yoinker: failed to fetch entries", vim.log.levels.ERROR)
      return
    end

    local entries = resp.Entries
    if #entries == 0 then
      vim.notify("yoinker: clipboard history is empty", vim.log.levels.INFO)
      return
    end

    M._open_picker(entries)
  end)
end

--- Format a relative timestamp
local function relative_time(timestamp)
  local now = os.time()
  local diff = now - timestamp
  if diff < 0 then diff = 0 end
  if diff < 60 then return diff .. "s" end
  if diff < 3600 then return math.floor(diff / 60) .. "m" end
  if diff < 86400 then return math.floor(diff / 3600) .. "h" end
  return math.floor(diff / 86400) .. "d"
end

--- Get preview text for an entry
local function get_preview(entry)
  local c = entry.content
  if c.type == "Text" then
    local s = c.text:gsub("\n", "\\n")
    if #s > 60 then s = s:sub(1, 60) .. "..." end
    return s
  elseif c.type == "Image" then
    return string.format("[image: %dx%d]", c.width or 0, c.height or 0)
  end
  return "[unknown]"
end

--- Build display lines from entries
local function build_lines(entries, query, selected)
  local filtered = {}
  local query_lower = query:lower()

  for i, e in ipairs(entries) do
    local preview = get_preview(e)
    if query == "" or preview:lower():find(query_lower, 1, true) then
      table.insert(filtered, { index = i, entry = e, preview = preview })
    end
  end

  -- Clamp selection
  if selected > #filtered then selected = #filtered end
  if selected < 1 then selected = 1 end

  local lines = {}
  for i, item in ipairs(filtered) do
    local pin = item.entry.pinned and " [pin]" or ""
    local time = relative_time(item.entry.timestamp)
    local marker = i == selected and "▸ " or "  "
    table.insert(lines, string.format("%s%4s | %s%s", marker, time, item.preview, pin))
  end

  return lines, filtered, selected
end

--- Open the floating picker window
function M._open_picker(entries)
  local query = ""
  local selected = 1
  local ns = vim.api.nvim_create_namespace("yoinker_picker")

  -- Create buffer
  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].buftype = "nofile"
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].swapfile = false

  -- Calculate window size
  local ui_width = vim.o.columns
  local ui_height = vim.o.lines
  local width = math.min(M.config.picker_width, ui_width - 4)
  local height = math.min(M.config.picker_height, #entries + 2, ui_height - 4)
  local row = math.floor((ui_height - height) / 2)
  local col = math.floor((ui_width - width) / 2)

  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor",
    width = width,
    height = height,
    row = row,
    col = col,
    style = "minimal",
    border = "rounded",
    title = " Yoinker ",
    title_pos = "center",
  })

  local function update()
    local lines, filtered, new_selected = build_lines(entries, query, selected)
    selected = new_selected

    -- Add search line at top
    table.insert(1, lines, "> " .. query)

    vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)

    -- Highlight selected line
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
    if #filtered > 0 then
      -- +1 for the search line at top
      vim.api.nvim_buf_add_highlight(buf, ns, "Visual", selected, 0, -1)
    end

    return filtered
  end

  local filtered = update()

  local function close()
    if vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end

  local function select_entry()
    if #filtered == 0 then
      close()
      return
    end
    local item = filtered[selected]
    if not item then
      close()
      return
    end
    close()

    -- Copy via daemon (sets system clipboard)
    send_request({ Copy = { index = item.index - 1 } }, function(resp)
      if resp and (resp == "Ok" or resp.Ok ~= nil) then
        -- Also paste into current buffer
        local content = item.entry.content
        if content.type == "Text" then
          vim.api.nvim_put(vim.split(content.text, "\n"), "", true, true)
        end
      end
    end)
  end

  -- Set up key handler via on_key or buffer keymaps
  local function on_key(key)
    if key == "\27" then -- Escape
      close()
    elseif key == "\r" then -- Enter
      select_entry()
    elseif key == "\8" or key == "\127" then -- Backspace
      query = query:sub(1, -2)
      selected = 1
      filtered = update()
    elseif key == "\14" then -- Ctrl+N / Down
      if selected < #filtered then
        selected = selected + 1
        filtered = update()
      end
    elseif key == "\16" then -- Ctrl+P / Up
      if selected > 1 then
        selected = selected - 1
        filtered = update()
      end
    elseif key == "\4" then -- Ctrl+D page down
      selected = math.min(selected + 10, #filtered)
      filtered = update()
    elseif key == "\21" then -- Ctrl+U page up
      selected = math.max(selected - 10, 1)
      filtered = update()
    elseif key == "\24" then -- Ctrl+X delete
      if #filtered > 0 and filtered[selected] then
        local item = filtered[selected]
        send_request({ Delete = { index = item.index - 1 } }, function(resp)
          if resp and (resp == "Ok" or resp.Ok ~= nil) then
            table.remove(entries, item.index)
            if #entries == 0 then
              close()
              vim.notify("yoinker: history cleared", vim.log.levels.INFO)
              return
            end
            filtered = update()
          end
        end)
      end
    elseif key:match("^[%g ]$") then -- printable character
      query = query .. key
      selected = 1
      filtered = update()
    end
  end

  -- Use vim.on_key for input capture in the floating window
  local capturing = true
  vim.on_key(function(raw_key)
    if not capturing then return end
    if not vim.api.nvim_win_is_valid(win) then
      capturing = false
      vim.on_key(nil, ns)
      return
    end
    -- Schedule to avoid issues with reentrancy
    vim.schedule(function()
      if not vim.api.nvim_win_is_valid(win) then
        capturing = false
        vim.on_key(nil, ns)
        return
      end
      on_key(raw_key)
    end)
    -- Return empty string to consume the key
    return ""
  end, ns)

  -- Also set up autocmd to clean up on buffer wipe
  vim.api.nvim_create_autocmd("BufWipeout", {
    buffer = buf,
    once = true,
    callback = function()
      capturing = false
      vim.on_key(nil, ns)
    end,
  })

  -- Set up arrow key mappings (they produce multi-byte sequences)
  vim.keymap.set("n", "<Up>", function()
    if selected > 1 then
      selected = selected - 1
      filtered = update()
    end
  end, { buffer = buf, nowait = true })

  vim.keymap.set("n", "<Down>", function()
    if selected < #filtered then
      selected = selected + 1
      filtered = update()
    end
  end, { buffer = buf, nowait = true })
end

--- Setup function for lazy.nvim / packer
function M.setup(opts)
  M.config = vim.tbl_deep_extend("force", M.config, opts or {})

  local prefix = M.config.keymap_prefix

  -- Keymaps
  vim.keymap.set("v", prefix .. "y", function()
    M.store_selection(false)
  end, { desc = "Yoinker: store selection" })
  vim.keymap.set("v", prefix .. "p", function()
    M.store_selection(true)
  end, { desc = "Yoinker: pin selection" })
  vim.keymap.set("n", prefix .. "l", function()
    M.list()
  end, { desc = "Yoinker: list history" })
  vim.keymap.set("n", prefix .. "1", function()
    M.paste(0)
  end, { desc = "Yoinker: paste most recent" })
end

return M
