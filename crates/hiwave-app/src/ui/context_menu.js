(function() {
    if (window.__hiwaveContextMenu) {
        return;
    }

    // Skip injection on about:blank or empty pages
    if (!document.body || !document.head) {
        return;
    }

    // Inject CSS for context menu
    const style = document.createElement('style');
    style.textContent = `
        .hiwave-context-menu {
            position: fixed;
            z-index: 2147483647;
            background: #1e293b;
            border: 1px solid #334155;
            border-radius: 10px;
            box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
            padding: 6px;
            color: #e2e8f0;
            display: flex;
            flex-direction: column;
            gap: 2px;
            min-width: 180px;
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            font-size: 13px;
            opacity: 0;
            visibility: hidden;
            pointer-events: none;
            transform: scale(0.95) translateY(-4px);
            transform-origin: top left;
            transition: opacity 0.15s ease, visibility 0.15s ease, transform 0.15s ease;
        }
        .hiwave-context-menu.visible {
            opacity: 1;
            visibility: visible;
            pointer-events: auto;
            transform: scale(1) translateY(0);
        }
        .hiwave-context-menu-item {
            background: transparent;
            border: none;
            border-radius: 6px;
            color: #94a3b8;
            text-align: left;
            padding: 8px 12px;
            cursor: pointer;
            display: flex;
            align-items: center;
            gap: 10px;
            transition: all 0.15s ease;
            width: 100%;
        }
        .hiwave-context-menu-item:hover {
            background: #334155;
            color: #f1f5f9;
        }
        .hiwave-context-menu-item:active {
            transform: scale(0.98);
        }
        .hiwave-context-menu-item.disabled {
            color: #475569;
            cursor: default;
            opacity: 0.5;
        }
        .hiwave-context-menu-item.disabled:hover {
            background: transparent;
        }
        .hiwave-context-menu-item .icon {
            width: 18px;
            text-align: center;
            font-size: 14px;
        }
        .hiwave-context-menu-divider {
            height: 1px;
            background: #334155;
            margin: 4px 8px;
        }
    `;
    document.head.appendChild(style);

    // Create menu element
    const menu = document.createElement('div');
    menu.className = 'hiwave-context-menu';
    document.body.appendChild(menu);

    let menuVisible = false;
    let currentContext = null;

    const handler = {
        detectContext(event) {
            const target = event.target;
            const selection = window.getSelection();
            const selectedText = selection ? selection.toString().trim() : '';

            let linkElement = target.closest('a[href]');
            let linkUrl = linkElement ? linkElement.href : null;

            let imageUrl = null;
            if (target.tagName === 'IMG' && target.src) {
                imageUrl = target.src;
            }

            return {
                x: event.clientX,
                y: event.clientY,
                linkUrl: linkUrl,
                imageUrl: imageUrl,
                selectedText: selectedText,
                pageUrl: location.href,
                pageTitle: document.title
            };
        },

        buildMenuItems(context) {
            const items = [];

            // Navigation items
            items.push(
                { label: 'Back', icon: '\u2190', action: 'go_back' },
                { label: 'Forward', icon: '\u2192', action: 'go_forward' },
                { label: 'Reload', icon: '\u21BB', action: 'reload' },
                { type: 'divider' }
            );

            // Selection items
            if (context.selectedText) {
                const truncated = context.selectedText.length > 25
                    ? context.selectedText.substring(0, 25) + '...'
                    : context.selectedText;
                items.push(
                    { label: 'Copy', icon: '\uD83D\uDCCB', action: 'copy_selection' },
                    { label: 'Search "' + truncated + '"', icon: '\uD83D\uDD0D', action: 'search_selection' },
                    { type: 'divider' }
                );
            }

            // Link items
            if (context.linkUrl) {
                items.push(
                    { label: 'Open Link in New Tab', icon: '\u2197', action: 'open_link_new_tab' },
                    { label: 'Copy Link', icon: '\uD83D\uDD17', action: 'copy_link' },
                    { type: 'divider' }
                );
            }

            // Image items
            if (context.imageUrl) {
                items.push(
                    { label: 'Open Image in New Tab', icon: '\uD83D\uDDBC', action: 'open_image_new_tab' },
                    { label: 'Copy Image URL', icon: '\uD83D\uDD17', action: 'copy_image_url' },
                    { type: 'divider' }
                );
            }

            // Always show these
            items.push(
                { label: 'Copy Page URL', icon: '\uD83D\uDD17', action: 'copy_page_url' },
                { label: 'Add to Shelf', icon: '\uD83D\uDCE5', action: 'shelf_active' }
            );

            return items;
        },

        renderMenu(items, x, y) {
            menu.innerHTML = items.map(item => {
                if (item.type === 'divider') {
                    return '<div class="hiwave-context-menu-divider"></div>';
                }
                const disabled = item.disabled ? ' disabled' : '';
                const icon = item.icon ? '<span class="icon">' + item.icon + '</span>' : '';
                return '<button class="hiwave-context-menu-item' + disabled + '" data-action="' + item.action + '">' +
                    icon + '<span>' + item.label + '</span></button>';
            }).join('');

            // Position menu
            const menuWidth = 200;
            const menuHeight = menu.offsetHeight || 300;
            const left = Math.max(8, Math.min(x, window.innerWidth - menuWidth - 8));
            const top = Math.max(8, Math.min(y, window.innerHeight - menuHeight - 8));
            menu.style.left = left + 'px';
            menu.style.top = top + 'px';

            // Show menu
            menu.classList.add('visible');
            menuVisible = true;

            // Add click handlers
            menu.querySelectorAll('.hiwave-context-menu-item').forEach(btn => {
                btn.onclick = (e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (btn.classList.contains('disabled')) return;
                    this.handleAction(btn.dataset.action);
                };
            });
        },

        hideMenu() {
            menu.classList.remove('visible');
            menuVisible = false;
            currentContext = null;
        },

        handleAction(action) {
            // Save context before hiding (hideMenu clears it)
            const ctx = currentContext;
            this.hideMenu();

            switch (action) {
                case 'go_back':
                    this.sendIpc('go_back');
                    break;
                case 'go_forward':
                    this.sendIpc('go_forward');
                    break;
                case 'reload':
                    location.reload();
                    break;
                case 'copy_selection':
                    if (ctx && ctx.selectedText) {
                        this.copyToClipboard(ctx.selectedText);
                    }
                    break;
                case 'search_selection':
                    if (ctx && ctx.selectedText) {
                        this.sendIpc('create_tab', { url: 'https://duckduckgo.com/?q=' + encodeURIComponent(ctx.selectedText) });
                    }
                    break;
                case 'open_link_new_tab':
                    if (ctx && ctx.linkUrl) {
                        this.sendIpc('create_tab', { url: ctx.linkUrl });
                    }
                    break;
                case 'copy_link':
                    if (ctx && ctx.linkUrl) {
                        this.copyToClipboard(ctx.linkUrl);
                    }
                    break;
                case 'open_image_new_tab':
                    if (ctx && ctx.imageUrl) {
                        this.sendIpc('create_tab', { url: ctx.imageUrl });
                    }
                    break;
                case 'copy_image_url':
                    if (ctx && ctx.imageUrl) {
                        this.copyToClipboard(ctx.imageUrl);
                    }
                    break;
                case 'copy_page_url':
                    this.copyToClipboard(location.href);
                    break;
                case 'shelf_active':
                    this.sendIpc('add_to_shelf', { tab_id: 'active' });
                    break;
            }
        },

        copyToClipboard(text) {
            navigator.clipboard.writeText(text).catch(() => {
                // Fallback for older browsers
                const textarea = document.createElement('textarea');
                textarea.value = text;
                textarea.style.position = 'fixed';
                textarea.style.opacity = '0';
                document.body.appendChild(textarea);
                textarea.select();
                document.execCommand('copy');
                document.body.removeChild(textarea);
            });
        },

        sendIpc(cmd, data) {
            if (window.ipc && window.ipc.postMessage) {
                const msg = data ? { cmd: cmd, ...data } : { cmd: cmd };
                window.ipc.postMessage(JSON.stringify(msg));
            }
        },

        handleContextMenu(event) {
            const target = event.target;
            const tagName = target.tagName;

            // Allow native context menu for form elements
            if (tagName === 'INPUT' || tagName === 'TEXTAREA' || target.isContentEditable) {
                return;
            }

            // Don't intercept if inside our own menu
            if (target.closest('.hiwave-context-menu')) {
                return;
            }

            event.preventDefault();
            event.stopPropagation();

            currentContext = this.detectContext(event);
            const items = this.buildMenuItems(currentContext);
            this.renderMenu(items, event.clientX, event.clientY);
        },

        init() {
            document.addEventListener('contextmenu', (e) => this.handleContextMenu(e), true);

            // Hide menu on click outside or Escape
            document.addEventListener('click', (e) => {
                if (menuVisible && !e.target.closest('.hiwave-context-menu')) {
                    this.hideMenu();
                }
            }, true);

            // Comprehensive keyboard shortcut handler for content WebView
            // Forwards browser shortcuts to the Rust backend
            document.addEventListener('keydown', (e) => {
                const isCtrl = e.ctrlKey || e.metaKey;
                const isShift = e.shiftKey;
                const isAlt = e.altKey;
                const ipc = window.ipc;

                if (!ipc || !ipc.postMessage) return;

                const send = (cmd, data = {}) => {
                    ipc.postMessage(JSON.stringify({ cmd, ...data }));
                };

                // Escape - close context menu and exit focus mode
                if (e.key === 'Escape') {
                    if (menuVisible) {
                        this.hideMenu();
                    }
                    send('exit_focus_mode');
                    return;
                }

                // Ctrl+Shift+F or F11 - Toggle focus mode
                if ((isCtrl && isShift && (e.key === 'f' || e.key === 'F')) || e.key === 'F11') {
                    e.preventDefault();
                    send('toggle_focus_mode');
                    return;
                }

                // Ctrl+T - New tab
                if (isCtrl && !isShift && (e.key === 't' || e.key === 'T')) {
                    e.preventDefault();
                    send('create_tab', { url: null });
                    return;
                }

                // Ctrl+W - Close tab
                if (isCtrl && !isShift && (e.key === 'w' || e.key === 'W')) {
                    e.preventDefault();
                    send('close_tab', { id: 'active' });
                    return;
                }

                // Ctrl+L - Focus address bar (notify chrome)
                if (isCtrl && !isShift && (e.key === 'l' || e.key === 'L')) {
                    e.preventDefault();
                    send('focus_address_bar');
                    return;
                }

                // Ctrl+K - Command palette
                if (isCtrl && !isShift && (e.key === 'k' || e.key === 'K')) {
                    e.preventDefault();
                    send('open_command_palette');
                    return;
                }

                // Ctrl+F - Find in page
                if (isCtrl && !isShift && (e.key === 'f' || e.key === 'F')) {
                    e.preventDefault();
                    send('open_find');
                    return;
                }

                // Ctrl+B - Toggle sidebar
                if (isCtrl && !isShift && (e.key === 'b' || e.key === 'B')) {
                    e.preventDefault();
                    send('toggle_sidebar');
                    return;
                }

                // Ctrl+Shift+S - Shelve tab
                if (isCtrl && isShift && (e.key === 's' || e.key === 'S')) {
                    e.preventDefault();
                    send('add_to_shelf', { tab_id: 'active' });
                    return;
                }

                // Ctrl+Shift+L - Autofill
                if (isCtrl && isShift && (e.key === 'l' || e.key === 'L')) {
                    e.preventDefault();
                    send('trigger_autofill');
                    return;
                }

                // Ctrl+R or F5 - Reload
                if ((isCtrl && !isShift && (e.key === 'r' || e.key === 'R')) || e.key === 'F5') {
                    e.preventDefault();
                    send('refresh');
                    return;
                }

                // Alt+Left - Go back
                if (isAlt && e.key === 'ArrowLeft') {
                    e.preventDefault();
                    send('go_back');
                    return;
                }

                // Alt+Right - Go forward
                if (isAlt && e.key === 'ArrowRight') {
                    e.preventDefault();
                    send('go_forward');
                    return;
                }

                // Ctrl+1-9 - Switch tabs
                if (isCtrl && !isShift && e.key >= '1' && e.key <= '9') {
                    e.preventDefault();
                    const index = parseInt(e.key) - 1;
                    send('activate_tab_by_index', { index });
                    return;
                }
            }, true);

            // Hide on scroll
            window.addEventListener('scroll', () => {
                if (menuVisible) {
                    this.hideMenu();
                }
            }, true);
        }
    };

    handler.init();
    window.__hiwaveContextMenu = handler;
})();
