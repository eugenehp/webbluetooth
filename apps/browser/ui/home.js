// The start page, and the shortest end-to-end check of the whole stack:
// shim → Tauri command → webbluetooth → CoreBluetooth → a radio in the room.

const log = document.getElementById('log');
const scan = document.getElementById('scan');

function say(line) {
  log.textContent += `${line}\n`;
  log.scrollTop = log.scrollHeight;
}

function fact(term, value, verdict) {
  const facts = document.getElementById('facts');
  const dt = document.createElement('dt');
  dt.textContent = term;
  const dd = document.createElement('dd');
  dd.textContent = value;
  if (verdict !== undefined) {
    dd.className = verdict ? 'yes' : 'no';
  }
  facts.append(dt, dd);
}

async function describe() {
  const present = 'bluetooth' in navigator;
  fact('navigator.bluetooth', present ? 'present' : 'absent', present);
  fact('secure context', String(window.isSecureContext), window.isSecureContext);
  fact('origin', window.location.origin);

  if (!present) {
    fact(
      'why',
      'The shim only installs on a secure context, as in any browser.',
      false
    );
    scan.disabled = true;
    return;
  }

  try {
    const available = await navigator.bluetooth.getAvailability();
    fact('adapter', available ? 'available' : 'unavailable', available);
  } catch (error) {
    fact('adapter', `${error.name}: ${error.message}`, false);
  }

  const granted = await navigator.bluetooth.getDevices();
  fact(
    'already permitted',
    granted.length
      ? granted.map((d) => d.name || d.id).join(', ')
      : 'nothing yet'
  );
}

scan.addEventListener('click', async () => {
  scan.disabled = true;
  try {
    say('requestDevice({ acceptAllDevices: true })…');
    const device = await navigator.bluetooth.requestDevice({
      acceptAllDevices: true,
      // `acceptAllDevices` grants no services on its own, so a device picked
      // this way can be connected to and nothing else. Naming a few common
      // ones is what makes the walk below have anything to show.
      optionalServices: [
        'generic_access',
        'device_information',
        'battery_service',
        'heart_rate',
      ],
    });
    say(`chose ${device.name || '(unnamed)'} — ${device.id}`);

    device.addEventListener('gattserverdisconnected', () => {
      say('gattserverdisconnected');
    });

    say('connecting…');
    const server = await device.gatt.connect();
    say(`connected: ${server.connected}`);

    const services = await server.getPrimaryServices();
    say(`${services.length} service(s) within the grant:`);
    for (const service of services) {
      say(`  ${service.uuid}`);
      const characteristics = await service.getCharacteristics();
      for (const characteristic of characteristics) {
        const can = Object.entries({
          read: characteristic.properties.read,
          write: characteristic.properties.write,
          writeNR: characteristic.properties.writeWithoutResponse,
          notify: characteristic.properties.notify,
          indicate: characteristic.properties.indicate,
        })
          .filter(([, yes]) => yes)
          .map(([name]) => name)
          .join(' ');
        say(`    ${characteristic.uuid}  [${can || 'no properties'}]`);

        if (characteristic.properties.read) {
          try {
            const value = await characteristic.readValue();
            say(`      = ${hex(value)}${ascii(value)}`);
          } catch (error) {
            say(`      read failed: ${error.name}`);
          }
        }
      }
    }
    say('done.');
  } catch (error) {
    say(`${error.name}: ${error.message}`);
  } finally {
    scan.disabled = false;
  }
});

function hex(view) {
  return [...new Uint8Array(view.buffer)]
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join(' ');
}

function ascii(view) {
  const bytes = new Uint8Array(view.buffer);
  const printable = [...bytes].every((b) => b >= 0x20 && b < 0x7f);
  return printable && bytes.length
    ? `   "${new TextDecoder().decode(bytes)}"`
    : '';
}

document.getElementById('clear').addEventListener('click', () => {
  log.textContent = '';
});

describe();
