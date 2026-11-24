    const { invoke } = window.__TAURI__.core;
    const { listen } = window.__TAURI__.event;
    const $ = (id) => document.getElementById(id);
    const MODEL = 'large-v3-turbo';
    const CHUNK_MS = 12000;

    function showError(e) {
      $('error').textContent = typeof e === 'string' ? e : (e && e.message) || JSON.stringify(e);
    }

    listen('transcribe://progress', (ev) => {
      const p = ev.payload.percent;
      $('progress').value = p;
      $('progress-text').textContent = p + '%';
    });

    // Cada bloco é gravado por um MediaRecorder próprio, para ser um arquivo completo e decodificável.
    let stream = null;
    let recording = false;

    async function sendChunk(blob, mime) {
      try {
        const data = Array.from(new Uint8Array(await blob.arrayBuffer()));
        const t = await invoke('transcribe_chunk', { data, mime, model: MODEL, language: 'auto' });
        $('out').textContent += (t.text || '') + '\n';
      } catch (e) { showError(e); }
    }

    function recordOne() {
      const rec = new MediaRecorder(stream);
      const parts = [];
      rec.ondataavailable = (ev) => { if (ev.data.size) parts.push(ev.data); };
      rec.onstop = () => {
        const mime = rec.mimeType || 'audio/webm';
        sendChunk(new Blob(parts, { type: mime }), mime);
        if (recording) recordOne();
        else if (stream) { stream.getTracks().forEach((t) => t.stop()); stream = null; }
      };
      rec.start();
      setTimeout(() => { if (rec.state !== 'inactive') rec.stop(); }, CHUNK_MS);
      recordOne.current = rec;
    }

    $('rec').addEventListener('click', async () => {
      $('error').textContent = '';
      if (recording) {
        recording = false;
        $('rec').textContent = 'Gravar';
        $('rec-status').textContent = '';
        if (recordOne.current && recordOne.current.state !== 'inactive') recordOne.current.stop();
        return;
      }
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch (e) { showError(e); return; }
      recording = true;
      $('rec').textContent = 'Parar';
      $('rec-status').textContent = 'Gravando...';
      recordOne();
    });

    $('pick').addEventListener('click', async () => {
      $('error').textContent = '';
      $('progress').value = 0;
      try {
        const path = await window.__TAURI__.dialog.open({ multiple: false });
        if (!path) return;
        const t = await invoke('transcribe_file', { path, model: MODEL, language: 'auto' });
        $('out').textContent += t.text + '\n';
      } catch (e) { showError(e); }
    });
