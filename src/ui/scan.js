'use strict';
// Pairing by QR code: camera frames, or a chosen picture, go to the panel as greyscale; the
// panel reads the code and pairs. The code's content never comes back to this page.
let scanStream = null;
let scanning = false;
const scanCanvas = document.createElement('canvas');

// One frame as the panel reads it: width and height (little-endian), then one byte per pixel.
function greyFrame(source, width, height, longest) {
  const scale = Math.min(1, longest / Math.max(width, height));
  const w = Math.max(1, Math.round(width * scale)), h = Math.max(1, Math.round(height * scale));
  scanCanvas.width = w; scanCanvas.height = h;
  const context = scanCanvas.getContext('2d', { willReadFrequently: true });
  context.drawImage(source, 0, 0, w, h);
  const rgba = context.getImageData(0, 0, w, h).data;
  const frame = new Uint8Array(8 + w * h), view = new DataView(frame.buffer);
  view.setUint32(0, w, true); view.setUint32(4, h, true);
  for (let i = 0, j = 8; i < rgba.length; i += 4, j++) frame[j] = (rgba[i] * 77 + rgba[i + 1] * 150 + rgba[i + 2] * 29) >> 8;
  return frame;
}
async function sendFrame(frame) {
  const response = await fetch('/api/scan', {
    method: 'POST', cache: 'no-store', body: frame,
    headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/octet-stream' }
  });
  const value = await response.json().catch(() => ({ error: t('unexpected') }));
  if (!response.ok) throw new Error((language === 'de' && serverDe[value.error]) || value.error || t('unexpected'));
  return value;
}
function stopCamera() {
  scanning = false;
  if (scanStream) { for (const track of scanStream.getTracks()) track.stop(); scanStream = null; }
  byId('qr-video').srcObject = null;
  byId('qr-scan').hidden = true; byId('qr-start').hidden = false;
}
async function scanned() {
  stopCamera(); forget(); wantPair = false; byId('code').value = '';
  await action(() => api('connect', {}));
}

byId('qr-camera').addEventListener('click', async () => {
  error('');
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) { error(t('qrNoCamera')); return; }
  try {
    scanStream = await navigator.mediaDevices.getUserMedia({ audio: false, video: { width: { ideal: 1280 }, height: { ideal: 720 } } });
  } catch (cause) {
    error(t(cause && cause.name === 'NotAllowedError' ? 'qrDenied' : 'qrNoCamera'));
    return;
  }
  const video = byId('qr-video');
  video.srcObject = scanStream;
  await video.play().catch(() => {});
  byId('qr-start').hidden = true; byId('qr-scan').hidden = false; byId('qr-status').textContent = t('qrHold');
  video.scrollIntoView({ block: 'center', behavior: 'smooth' });
  scanning = true;
  while (scanning) {
    if (video.readyState >= 2 && video.videoWidth) {
      try {
        const result = await sendFrame(greyFrame(video, video.videoWidth, video.videoHeight, 800));
        if (result.paired) { await scanned(); return; }
      } catch (cause) {
        // Another QR code, or a pairing that failed: say so and keep looking.
        if (scanning) byId('qr-status').textContent = cause.message;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
});
byId('qr-cancel').addEventListener('click', stopCamera);
// The camera is off whenever the page or the pairing step is out of sight.
document.addEventListener('visibilitychange', () => { if (document.hidden && scanning) stopCamera(); });
new MutationObserver(() => { if (byId('step-pair').hidden && scanning) stopCamera(); })
  .observe(byId('step-pair'), { attributes: true, attributeFilter: ['hidden'] });

byId('qr-file').addEventListener('change', async () => {
  const file = byId('qr-file').files[0];
  byId('qr-file').value = '';
  if (!file) return;
  error('');
  try {
    const picture = await createImageBitmap(file);
    const result = await sendFrame(greyFrame(picture, picture.width, picture.height, 2000));
    if (result.paired) await scanned(); else error(t('qrNotFound'));
  } catch (cause) {
    error(cause instanceof DOMException ? t('qrNotFound') : cause.message || t('unexpected'));
  }
});
