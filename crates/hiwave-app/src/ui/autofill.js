// HiWave Autofill - password form detection and filling
(function() {
    if (window.hiwaveAutofill) return;

    let autofillPopup = null;
    let currentPasswordField = null;

    window.hiwaveAutofill = {
        // Trigger autofill - request credentials for current domain
        trigger: function() {
            const passwordField = document.querySelector('input[type="password"]:not([disabled])');
            if (!passwordField) {
                return;
            }
            currentPasswordField = passwordField;

            // Request credentials for this domain
            const domain = window.location.hostname;
            if (window.ipc && window.ipc.postMessage) {
                window.ipc.postMessage(JSON.stringify({
                    cmd: 'get_credentials_for_autofill',
                    domain: domain
                }));
            }
        },

        // Show credential selection popup
        showCredentials: function(credentials) {
            if (!credentials || credentials.length === 0) {
                return;
            }

            // Remove existing popup
            this.hidePopup();

            // Create popup
            autofillPopup = document.createElement('div');
            autofillPopup.id = 'hiwave-autofill-popup';
            autofillPopup.style.cssText = `
                position: fixed;
                z-index: 2147483647;
                background: #1e293b;
                border: 1px solid #334155;
                border-radius: 8px;
                box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
                padding: 8px;
                min-width: 250px;
                max-width: 350px;
                font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
                font-size: 14px;
            `;

            // Position near password field
            if (currentPasswordField) {
                const rect = currentPasswordField.getBoundingClientRect();
                autofillPopup.style.top = (rect.bottom + 4) + 'px';
                autofillPopup.style.left = rect.left + 'px';
            } else {
                autofillPopup.style.top = '20%';
                autofillPopup.style.left = '50%';
                autofillPopup.style.transform = 'translateX(-50%)';
            }

            // Add title
            const title = document.createElement('div');
            title.style.cssText = `
                color: #94a3b8;
                font-size: 12px;
                margin-bottom: 8px;
                padding: 0 4px;
            `;
            title.textContent = 'Choose a credential';
            autofillPopup.appendChild(title);

            // Add credential options
            credentials.forEach(cred => {
                const item = document.createElement('div');
                item.style.cssText = `
                    padding: 10px 12px;
                    border-radius: 6px;
                    cursor: pointer;
                    color: #f1f5f9;
                    transition: background 0.1s;
                `;
                item.onmouseenter = () => item.style.background = '#334155';
                item.onmouseleave = () => item.style.background = 'transparent';
                item.innerHTML = `
                    <div style="font-weight: 500;">${this.escapeHtml(cred.username)}</div>
                    <div style="color: #64748b; font-size: 12px;">${this.escapeHtml(cred.domain)}</div>
                `;
                item.onclick = () => {
                    this.fill(cred.username, cred.password);
                    this.hidePopup();
                };
                autofillPopup.appendChild(item);
            });

            document.body.appendChild(autofillPopup);

            // Close on outside click
            setTimeout(() => {
                document.addEventListener('click', this.handleOutsideClick, true);
            }, 100);
        },

        handleOutsideClick: function(e) {
            if (autofillPopup && !autofillPopup.contains(e.target)) {
                window.hiwaveAutofill.hidePopup();
            }
        },

        hidePopup: function() {
            if (autofillPopup) {
                autofillPopup.remove();
                autofillPopup = null;
            }
            document.removeEventListener('click', this.handleOutsideClick, true);
        },

        // Fill credentials into the form
        fill: function(username, password) {
            // Find password field
            const passwordField = currentPasswordField ||
                document.querySelector('input[type="password"]:not([disabled])');
            if (!passwordField) {
                return;
            }

            // Find username field (usually before password field)
            let usernameField = null;

            // Try to find by common attributes
            const form = passwordField.closest('form');
            const container = form || document;

            const usernameSelectors = [
                'input[type="email"]:not([disabled])',
                'input[type="text"][name*="user"]:not([disabled])',
                'input[type="text"][name*="email"]:not([disabled])',
                'input[type="text"][name*="login"]:not([disabled])',
                'input[type="text"][autocomplete="username"]:not([disabled])',
                'input[type="text"][autocomplete="email"]:not([disabled])',
                'input[type="text"]:not([disabled])'
            ];

            for (const selector of usernameSelectors) {
                usernameField = container.querySelector(selector);
                if (usernameField && usernameField !== passwordField) break;
            }

            // Fill the fields
            if (usernameField) {
                this.setInputValue(usernameField, username);
            }
            this.setInputValue(passwordField, password);
        },

        // Set input value and trigger events for React/Vue compatibility
        setInputValue: function(input, value) {
            // Set native value
            const nativeInputValueSetter = Object.getOwnPropertyDescriptor(
                window.HTMLInputElement.prototype, 'value'
            ).set;
            nativeInputValueSetter.call(input, value);

            // Dispatch events
            input.dispatchEvent(new Event('input', { bubbles: true }));
            input.dispatchEvent(new Event('change', { bubbles: true }));
        },

        escapeHtml: function(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
        }
    };

    // Keyboard shortcut: Ctrl+Shift+L (or Cmd+Shift+L on Mac) triggers autofill
    document.addEventListener('keydown', function(e) {
        const isCtrl = e.ctrlKey || e.metaKey;
        if (isCtrl && e.shiftKey && (e.key === 'l' || e.key === 'L')) {
            e.preventDefault();
            e.stopPropagation();
            window.hiwaveAutofill.trigger();
        }
    }, true); // Use capture phase to get event first

    console.log('[HiWave Autofill] Initialized');
})();
