// --- edit state ---

let editId = null;
let editObjectsUrl = null;

const EXPIRY_DURATIONS = {
    '1h':  60 * 60 * 1000,
    '4h':  4 * 60 * 60 * 1000,
    '1d':  24 * 60 * 60 * 1000,
    '1w':  7 * 24 * 60 * 60 * 1000,
    '1mo': 30 * 24 * 60 * 60 * 1000,
    '1y':  365 * 24 * 60 * 60 * 1000,
};

// --- form-section generic functions ---

function enableSection(section) {
    const checkbox = section.querySelector('input[type=checkbox]');
    if (!checkbox.checked) {
        checkbox.checked = true;
        updateSectionStateFor(section);
    }
}

function updateSectionState(event) {
    const section = event.target.closest('.form-section');
    if (section) updateSectionStateFor(section);
}

function updateSectionStateFor(section) {
    const checkbox = section.querySelector('input[type=checkbox]');
    const enabled = checkbox.checked;
    section.querySelectorAll('input:not([type=checkbox]), button').forEach(el => {
        el.classList.toggle('inactive', !enabled);
    });
}

function generateKey(event) {
    event.preventDefault();
    const section = event.target.closest('.form-section');
    const input = section.querySelector('input[type=text]');
    input.value = randomKey();
    enableSection(section);
}

function getUnlistedKey(section) {
    const checkbox = section.querySelector('input[type=checkbox]');
    if (!checkbox.checked) return null;
    return section.querySelector('input[type=text]').value || null;
}

function computeExpiry(section) {
    const checkbox = section.querySelector('input[type=checkbox]');
    if (!checkbox.checked) return null;
    const selected = section.querySelector('input[type=radio]:checked');
    if (!selected) return null;
    const val = selected.value;
    if (val === 'custom') {
        const dt = section.querySelector('.expiry-custom').value;
        return dt ? new Date(dt).toISOString() : null;
    }
    return new Date(Date.now() + EXPIRY_DURATIONS[val]).toISOString();
}

// --- edit overlay ---

function openEdit(button) {
    editId = button.getAttribute('data_id');
    editObjectsUrl = button.getAttribute('data_objects_url');
    const key = button.getAttribute('data_key');
    const expires = button.getAttribute('data_expires');

    const overlay = document.getElementById('edit-overlay');
    document.getElementById('edit-title').textContent = editId;
    document.getElementById('edit-result').textContent = '';

    // Populate unlisted section
    const sections = overlay.querySelectorAll('.form-section');
    const unlistedSection = sections[0];
    const expirySection = sections[1];

    const unlistedCheckbox = unlistedSection.querySelector('input[type=checkbox]');
    const keyInput = unlistedSection.querySelector('input[type=text]');
    unlistedCheckbox.checked = key !== '';
    keyInput.value = key;
    updateSectionStateFor(unlistedSection);

    // Populate expiry section
    const expiryCheckbox = expirySection.querySelector('input[type=checkbox]');
    // Clear all radios first
    expirySection.querySelectorAll('input[type=radio]').forEach(r => r.checked = false);
    if (expires) {
        expiryCheckbox.checked = true;
        const dt = new Date(expires);
        const local = new Date(dt.getTime() - dt.getTimezoneOffset() * 60000)
            .toISOString()
            .slice(0, 16);
        expirySection.querySelector('.expiry-custom').value = local;
        expirySection.querySelector('input[type=radio][value=custom]').checked = true;
    } else {
        expiryCheckbox.checked = false;
        expirySection.querySelector('.expiry-custom').value = '';
    }
    updateSectionStateFor(expirySection);

    document.body.classList.add("covered");
}

function closeEdit() {
    document.body.classList.remove("covered");
    editId = null;
    editObjectsUrl = null;
}

function editBackgroundClick(event) {
    if (event.target === event.currentTarget) {
        closeEdit();
    }
}

function copyItemUrl(button) {
    const li = button.closest('li');
    const a = li.querySelector('a');
    navigator.clipboard.writeText(a.href);
}

async function editSave() {
    const overlay = document.getElementById('edit-overlay');
    const sections = overlay.querySelectorAll('.form-section');
    const unlistedKey = getUnlistedKey(sections[0]);
    const expires = computeExpiry(sections[1]);

    const resp = await fetch(editObjectsUrl, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ unlisted_key: unlistedKey, expires }),
    });

    if (resp.ok) {
        location.reload();
    } else {
        document.getElementById('edit-result').textContent = 'Save failed: ' + resp.status;
    }
}

async function editDelete() {
    const resp = await fetch(editObjectsUrl, { method: 'DELETE' });
    if (resp.ok) {
        location.reload();
    } else {
        document.getElementById('edit-result').textContent = 'Delete failed: ' + resp.status;
    }
}

function randomKey() {
    const buf = new Uint8Array(4);
    crypto.getRandomValues(buf);
    return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('');
}

// --- form-section wiring (all sections on the page) ---

document.querySelectorAll('.form-section').forEach(section => {
    const checkbox = section.querySelector('input[type=checkbox]');
    checkbox.addEventListener('change', updateSectionState);

    // Focus-enables-checkbox: focusing any non-checkbox input or clicking any button enables the section
    section.querySelectorAll('input:not([type=checkbox])').forEach(input => {
        input.addEventListener('focus', () => enableSection(section));
    });
    section.querySelectorAll('button').forEach(btn => {
        btn.addEventListener('click', () => enableSection(section));
    });

    // For expiry sections: wire up custom datetime-local focus to select custom radio
    const customInput = section.querySelector('.expiry-custom');
    if (customInput) {
        customInput.addEventListener('focus', () => {
            const customRadio = section.querySelector('input[type=radio][value=custom]');
            if (customRadio) customRadio.checked = true;
        });
    }

    updateSectionStateFor(section);
});

// --- add form ---

const form = document.getElementById('add-form');
const fileInput = form.querySelector('[name=file]');
const pathInput = form.querySelector('[name=path]');
const idInput = document.getElementById('object-id');
const overrideCheckbox = form.querySelector('[name=override-id]');

function derivedId() {
    const mode = form.querySelector('[name=mode]:checked').value;
    if (mode === 'upload') {
        return fileInput.files[0] ? fileInput.files[0].name : '';
    } else {
        const parts = pathInput.value.split('/').filter(p => p.length > 0);
        return parts.length > 0 ? parts[parts.length - 1] : '';
    }
}

function syncId() {
    if (!overrideCheckbox.checked) {
        idInput.value = derivedId();
    }
}

overrideCheckbox.addEventListener('change', () => {
    if (!overrideCheckbox.checked) idInput.value = derivedId();
});

function setMode(mode) {
    const uploadMode = mode === 'upload';
    fileInput.classList.toggle('inactive', !uploadMode);
    pathInput.classList.toggle('inactive', uploadMode);
    syncId();
}

form.querySelectorAll('[name=mode]').forEach(radio => {
    radio.addEventListener('change', () => setMode(radio.value));
});

fileInput.addEventListener('click', () => {
    form.querySelector('[name=mode][value=upload]').checked = true;
    setMode('upload');
});

pathInput.addEventListener('focus', () => {
    form.querySelector('[name=mode][value=link]').checked = true;
    setMode('link');
});

fileInput.addEventListener('change', syncId);
pathInput.addEventListener('input', syncId);

form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const id = idInput.value;
    const mode = form.querySelector('[name=mode]:checked').value;
    const result = form.querySelector('.result');

    if (mode === 'upload' && !fileInput.files[0]) {
        result.textContent = 'Please select a file.';
        return;
    }
    if (mode === 'link' && !pathInput.value) {
        result.textContent = 'Please enter a path.';
        return;
    }

    const sections = form.querySelectorAll('.form-section');
    const unlistedKey = getUnlistedKey(sections[1]);
    const expires = computeExpiry(sections[2]);

    let params = [];
    if (mode === 'link') params.push('link=' + encodeURIComponent(pathInput.value));
    if (unlistedKey) params.push('unlisted_key=' + encodeURIComponent(unlistedKey));
    if (expires) params.push('expires=' + encodeURIComponent(expires));

    let url = 'objects/' + encodeURIComponent(id);
    if (params.length > 0) url += '?' + params.join('&');

    let fetchOptions = { method: 'PUT' };
    if (mode === 'upload') {
        fetchOptions.body = fileInput.files[0];
    }

    const resp = await fetch(url, fetchOptions);
    if (resp.ok) {
        location.reload();
    } else {
        const body = await resp.text();
        result.textContent = (mode === 'upload' ? 'Upload' : 'Link') + ' failed: ' + body;
    }
});
