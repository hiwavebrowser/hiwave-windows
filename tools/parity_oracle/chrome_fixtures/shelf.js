// shelf.js — board state for shelf.html (CH3.A1): the command palette with the
// commands the host returns for an empty query, through hiwaveShelf.showCommands.
(function () {
  var board = window.__hiwaveBoard;
  var commands = [
    { id: 'new-tab', label: 'New Tab', category: 'Tabs', shortcut: 'Ctrl+T' },
    { id: 'shelve-tab', label: 'Shelve Tab', category: 'Shelf', shortcut: 'Ctrl+Shift+S' },
    { id: 'toggle-shield', label: 'Toggle Shield', category: 'Privacy', shortcut: '' },
    { id: 'open-settings', label: 'Open Settings', category: 'App', shortcut: 'Ctrl+,' }
  ];
  board.on('search_commands', function (msg) {
    var q = String(msg.query || '').toLowerCase();
    window.hiwaveShelf.showCommands(commands.filter(function (c) {
      return !q || c.label.toLowerCase().indexOf(q) !== -1 || c.category.toLowerCase().indexOf(q) !== -1;
    }));
  });
  window.hiwaveShelf.showCommands(commands);
})();
