-- An isolated real Neovim + Blink + LSP process test; no user config is loaded.
local function check()
  local plugin = assert(os.getenv('BLINK_PLUGIN'), 'BLINK_PLUGIN must point at a Blink checkout')
  local server = assert(os.getenv('PKL_LSP_BINARY'), 'PKL_LSP_BINARY must point at the built binary')
  local fixture = assert(os.getenv('PKL_FIXTURE'), 'PKL_FIXTURE must point at the public fixture')
  vim.opt.runtimepath:prepend(plugin)
  local blink = require('blink.cmp')
  blink.setup({
    keymap = { preset = 'none' },
    sources = { default = { 'lsp' } },
    completion = { menu = { auto_show = false } },
    fuzzy = { implementation = 'lua' },
  })
  vim.cmd.edit(fixture)
  vim.bo.filetype = 'pkl'
  vim.api.nvim_win_set_cursor(0, { 2, #vim.api.nvim_buf_get_lines(0, 1, 2, false)[1] })
  local client_id = assert(vim.lsp.start({ name = 'pkl-lsp-rs', cmd = { server }, filetypes = { 'pkl' } }))
  assert(vim.wait(20000, function()
    local client = vim.lsp.get_client_by_id(client_id)
    return client and client.initialized
  end, 25), 'LSP did not initialize')
  vim.cmd.startinsert()
  local found = false
  for _ = 1, 3 do
    blink.show({ providers = { 'lsp' } })
    found = vim.wait(18000, function()
      for _, item in ipairs(blink.get_items()) do
        if item.label == 'min_hk_version' then return true end
      end
      return false
    end, 25)
    if found then break end
    blink.hide()
    vim.wait(100)
  end
  assert(found, 'Blink did not present min_hk_version from the public hk package')
  assert(blink.is_menu_visible(), 'Blink completion menu was not visible')
  print('PASS: Neovim + Blink showed min_hk_version from the public hk package using the Rust LSP')
  vim.lsp.get_client_by_id(client_id):stop()
end

local ok, err = pcall(check)
if not ok then
  io.stderr:write('FAIL: ' .. tostring(err) .. '\n')
  vim.cmd('cquit 1')
else
  vim.cmd('qa!')
end
