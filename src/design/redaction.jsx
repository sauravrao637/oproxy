const SENSITIVE_KEY_PATTERNS = [
  /^authorization$/i,
  /^cookie$/i,
  /^set-cookie$/i,
  /^x-api-key$/i,
  /api[_-]?key/i,
  /access[_-]?token/i,
  /refresh[_-]?token/i,
  /password/i,
  /secret/i,
  /^token$/i,
];

const REDACTED = '••••••';

function isSensitiveKey(key) {
  return SENSITIVE_KEY_PATTERNS.some(re => re.test(String(key || '')));
}

function redactValueByKey(key, value) {
  return isSensitiveKey(key) ? REDACTED : value;
}

function redactHeaders(headers) {
  return Object.fromEntries(Object.entries(headers || {}).map(([k, v]) => [k, redactValueByKey(k, String(v))]));
}

function redactJsonValue(value, key = '') {
  if (isSensitiveKey(key)) return REDACTED;
  if (Array.isArray(value)) return value.map(v => redactJsonValue(v));
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, redactJsonValue(v, k)]));
  }
  return value;
}

function redactBodyText(body, contentType = '') {
  if (!body) return body || '';
  const text = String(body);
  if (contentType.toLowerCase().includes('json') || /^[\s\r\n]*[\[{]/.test(text)) {
    try {
      return JSON.stringify(redactJsonValue(JSON.parse(text)), null, 2);
    } catch {}
  }
  let out = text;
  for (const re of SENSITIVE_KEY_PATTERNS) {
    out = out.replace(new RegExp(`(${re.source})(["'\\s:=]+)([^&\\s"',}]+)`, 'ig'), `$1$2${REDACTED}`);
  }
  return out;
}

Object.assign(window, {
  REDACTED,
  isSensitiveKey,
  redactHeaders,
  redactBodyText,
  redactValueByKey,
});
