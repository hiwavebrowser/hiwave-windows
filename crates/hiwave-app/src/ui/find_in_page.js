(function() {
    if (window.__hiwaveFind) {
        return;
    }

    const matchClass = 'zen-find-match';
    const activeClass = 'active';
    const styleId = 'zen-find-style';
    const skipTags = ['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'TEXTAREA', 'INPUT'];

    function installStyle() {
        if (document.getElementById(styleId)) {
            return;
        }
        const style = document.createElement('style');
        style.id = styleId;
        style.textContent = `
            .${matchClass} {
                background: rgba(6, 182, 212, 0.35);
                border-radius: 3px;
                padding: 0 1px;
            }
            .${matchClass}.${activeClass} {
                background: rgba(6, 182, 212, 0.9);
                box-shadow: 0 0 0 2px rgba(6, 182, 212, 0.5);
            }
        `;
        document.head.appendChild(style);
    }

    function escapeRegExp(value) {
        return value.replace(/[.*+?^${}()|[\\]\\]/g, '\\$&');
    }

    // Take a mark out and put its text back where it came from. highlightRange splits the
    // matched text node into <node>, <mark>, <tail>, so the mark's neighbours are text nodes
    // the find created: fold the mark and the tail back into the first, and the page's own
    // text nodes (and their identity) are left as they were. Nothing is normalized.
    function unwrap(mark) {
        const parent = mark.parentNode;
        if (!parent) {
            return;
        }
        const text = mark.textContent || '';
        const prev = mark.previousSibling;
        const next = mark.nextSibling;
        if (prev && prev.nodeType === 3) {
            prev.data += text;
            parent.removeChild(mark);
            if (next && next.nodeType === 3) {
                prev.data += next.data;
                parent.removeChild(next);
            }
        } else if (next && next.nodeType === 3) {
            next.data = text + next.data;
            parent.removeChild(mark);
        } else {
            parent.replaceChild(document.createTextNode(text), mark);
        }
    }

    function clearHighlights() {
        Array.from(document.querySelectorAll(`.${matchClass}`)).forEach(unwrap);
    }

    function highlightRange(node, start, end) {
        const range = document.createRange();
        range.setStart(node, start);
        range.setEnd(node, end);
        const mark = document.createElement('span');
        mark.className = matchClass;
        range.surroundContents(mark);
        range.detach();
        return mark;
    }

    function highlightDocumentMatches(query, caseSensitive) {
        installStyle();
        clearHighlights();
        const regex = new RegExp(escapeRegExp(query), caseSensitive ? 'g' : 'gi');
        const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
        const matchesByNode = [];
        let current;

        while ((current = walker.nextNode())) {
            const parent = current.parentElement;
            if (!current.nodeValue) {
                continue;
            }
            if (parent && skipTags.includes(parent.tagName)) {
                continue;
            }
            const text = current.nodeValue;
            regex.lastIndex = 0;
            const ranges = [];
            let match;
            while ((match = regex.exec(text)) !== null) {
                if (match[0].length === 0) {
                    regex.lastIndex += 1;
                    continue;
                }
                ranges.push({ start: match.index, end: match.index + match[0].length });
                if (match.index === regex.lastIndex) {
                    regex.lastIndex += 1;
                }
            }
            if (ranges.length) {
                matchesByNode.push({ node: current, ranges });
            }
        }

        matchesByNode.forEach(({ node: textNode, ranges }) => {
            ranges.sort((a, b) => b.start - a.start);
            const marks = [];
            ranges.forEach(({ start, end }) => {
                const mark = highlightRange(textNode, start, end);
                marks.unshift(mark);
            });
            window.__hiwaveFind.matches.push(...marks);
        });

    }

    function scrollToMatch(element) {
        if (!element) {
            return;
        }
        element.scrollIntoView({ block: 'center', behavior: 'auto' });
    }

    const zenFind = {
        matches: [],
        currentIndex: -1,
        lastQuery: '',
        lastCase: false,
        ensureStyle() {
            installStyle();
        },
        clear() {
            clearHighlights();
            this.matches = [];
            this.currentIndex = -1;
            this.lastQuery = '';
            this.lastCase = false;
        },
        highlightMatches(query, caseSensitive) {
            this.matches = [];
            this.currentIndex = -1;
            highlightDocumentMatches(query, caseSensitive);
        },
        navigate(direction) {
            const total = this.matches.length;
            if (total === 0) {
                this.currentIndex = -1;
                return;
            }
            if (direction === 'next') {
                this.currentIndex = (this.currentIndex + 1 + total) % total;
            } else if (direction === 'prev') {
                    this.currentIndex = (this.currentIndex - 1 + total) % total;
            } else if (this.currentIndex === -1) {
                this.currentIndex = 0;
            }
            this.matches.forEach((match, idx) => {
                match.classList.toggle(activeClass, idx === this.currentIndex);
            });
            scrollToMatch(this.matches[this.currentIndex]);
        },
        run(action) {
            this.ensureStyle();
            const query = (action && action.query) || '';
            const caseSensitive = !!(action && action.case_sensitive);
            const direction = (action && action.direction) || 'reset';
            if (!query) {
                this.clear();
                return { count: 0, index: -1, query: '' };
            }
            const changed = direction === 'reset' || this.lastQuery !== query || this.lastCase !== caseSensitive;
            if (changed) {
                this.lastQuery = query;
                this.lastCase = caseSensitive;
                this.highlightMatches(query, caseSensitive);
            }
            if (this.matches.length === 0) {
                return { count: 0, index: -1, query };
            }
            if (direction === 'next') {
                this.navigate('next');
            } else if (direction === 'prev') {
                this.navigate('prev');
            } else {
                this.navigate('current');
            }
            return { count: this.matches.length, index: this.currentIndex, query };
        },
    };

    window.__hiwaveFind = zenFind;
})();
