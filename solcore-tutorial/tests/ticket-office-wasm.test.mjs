import assert from 'node:assert/strict';
import test from 'node:test';
import { backends, initializeBrowserWasm } from '../../scripts/test-support/browser-wasm.mjs';
import { loadProjects } from '../scripts/projects.mjs';

const { run } = await initializeBrowserWasm();
const projects = await loadProjects();
// The first three funded Playground accounts. The initial call deploys as owner.
const owner = '0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266';
const guest = '0x70997970C51812dc3A010C7d01b50e0d17dc79C8';
const otherGuest = '0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC';
const maxUint256 = ((1n << 256n) - 1n).toString();

function session(backend, sandboxEpoch, edit = file => file) {
  const project = projects.find(candidate => candidate.id === 'tutorial-ticket-office');
  assert.ok(project, 'TicketOffice solution must be in the tutorial catalog');
  const input = {
    files: project.files.map(edit),
    entry: project.entry,
    options: { backend, emitYul: true, emitHull: true, emitAbi: true },
    sandboxEpoch,
  };
  const invoke = (signature, args = [], caller = owner, status = 'success') => {
    const result = run({ ...input, manual: {
      contract: 'TicketOffice', signature, arguments: JSON.stringify(args),
      constructorArguments: '[]', caller, simulate: false,
    } });
    assert.equal(result.success, true, JSON.stringify(result.diagnostics));
    assert.equal(result.execution?.status, status, `${signature}: ${JSON.stringify(result.execution)}`);
    return result.execution.decoded;
  };
  assert.equal(invoke('capacity()'), '12');
  const booking = caller => ({
    seats: invoke('mySeats()', [], caller),
    quote: invoke('myQuote()', [], caller),
  });
  return { invoke, booking };
}

for (const [index, backend] of backends.entries()) {
  test(`${backend}: TicketOffice keeps callers' bookings and cancellation separate`, () => {
    const { invoke, booking } = session(backend, 128120 + index * 10);
    assert.equal(invoke('quoteStandard(uint256)', [4]), '40');
    assert.equal(invoke('quotePremium(uint256)', [4], guest), '60');
    assert.equal(invoke('remaining()'), '12');
    assert.deepEqual(booking(owner), { seats: '0', quote: '0' });
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });

    assert.equal(invoke('reserveStandard(uint256)', [2]), '20');
    assert.equal(invoke('reservePremium(uint256)', [3], guest), '45');
    assert.equal(invoke('remaining()'), '7');
    assert.deepEqual(booking(owner), { seats: '2', quote: '20' });
    assert.deepEqual(booking(guest), { seats: '3', quote: '45' });

    invoke('reservePremium(uint256)', [1], owner, 'revert');
    assert.deepEqual(booking(owner), { seats: '2', quote: '20' });
    assert.equal(invoke('remaining()'), '7');

    assert.equal(invoke('cancel()', [], guest), '45');
    assert.equal(invoke('remaining()'), '10');
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });
    assert.deepEqual(booking(owner), { seats: '2', quote: '20' });
    invoke('cancel()', [], guest, 'revert');
    assert.equal(invoke('remaining()'), '10');

    assert.equal(invoke('reserveStandard(uint256)', [1], guest), '10');
    assert.equal(invoke('cancel()'), '20');
    assert.deepEqual(booking(owner), { seats: '0', quote: '0' });
    assert.deepEqual(booking(guest), { seats: '1', quote: '10' });
    assert.equal(invoke('remaining()'), '11');
  });

  test(`${backend}: TicketOffice rejects invalid quantities and admits exactly its capacity`, () => {
    const { invoke, booking } = session(backend, 128121 + index * 10);
    for (const signature of ['reserveStandard(uint256)', 'reservePremium(uint256)',
      'quoteStandard(uint256)', 'quotePremium(uint256)']) {
      for (const quantity of [0, 13, maxUint256]) {
        invoke(signature, [quantity], guest, 'revert');
      }
    }
    assert.equal(invoke('remaining()'), '12');
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });

    assert.equal(invoke('reservePremium(uint256)', [12], guest), '180');
    assert.equal(invoke('remaining()'), '0');
    invoke('reserveStandard(uint256)', [1], owner, 'revert');
    assert.deepEqual(booking(owner), { seats: '0', quote: '0' });
    assert.deepEqual(booking(guest), { seats: '12', quote: '180' });
    assert.equal(invoke('remaining()'), '0');

    assert.equal(invoke('cancel()', [], guest), '180');
    assert.equal(invoke('remaining()'), '12');
    assert.equal(invoke('reserveStandard(uint256)', [12]), '120');
    invoke('reservePremium(uint256)', [1], guest, 'revert');
    assert.deepEqual(booking(owner), { seats: '12', quote: '120' });
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });
    assert.equal(invoke('remaining()'), '0');
  });

  test(`${backend}: only the TicketOffice organizer can close and closing preserves bookings`, () => {
    const { invoke, booking } = session(backend, 128122 + index * 10);
    assert.equal(invoke('reservePremium(uint256)', [2], guest), '30');
    invoke('close()', [], guest, 'revert');
    assert.equal(invoke('isOpen()'), 'true');

    invoke('close()');
    assert.equal(invoke('isOpen()'), 'false');
    invoke('close()', [], owner, 'revert');
    invoke('reserveStandard(uint256)', [1], owner, 'revert');
    invoke('reservePremium(uint256)', [1], otherGuest, 'revert');
    invoke('cancel()', [], guest, 'revert');
    assert.equal(invoke('quoteStandard(uint256)', [2]), '20');
    assert.equal(invoke('quotePremium(uint256)', [2], guest), '30');
    assert.deepEqual(booking(owner), { seats: '0', quote: '0' });
    assert.deepEqual(booking(otherGuest), { seats: '0', quote: '0' });
    assert.deepEqual(booking(guest), { seats: '2', quote: '30' });
    assert.equal(invoke('remaining()'), '10');
    assert.equal(invoke('isOpen()'), 'false');
  });

  test(`${backend}: the Supporter exercise reuses quoting, booking, and cancellation`, () => {
    const edit = file => {
      if (file.path === 'pricing.sol') {
        return { ...file, content: file.content.replace('Standard(*), Premium(*)',
          'Standard(*), Premium(*), Supporter(*)') + `
enum Supporter {
    Supporter
}

impl Priced<Supporter> {
    function unitPrice(ticket: Supporter) returns (uint256) {
        return uint256(25);
    }
}
` };
      }
      if (file.path === 'TicketOffice.sol') {
        return { ...file, content: file.content
          .replace('import { Standard, Premium, quote }', 'import { Standard, Premium, Supporter, quote }')
          .replace(/\n}\s*$/, `
    function quoteSupporter(quantity: uint256) public returns (uint256) {
        require(quantity > uint256(0), "choose at least one seat");
        require(quantity <= capacity(), "quantity exceeds capacity");
        return quote(Supporter, quantity);
    }

    function reserveSupporter(quantity: uint256) public returns (uint256) {
        validate(quantity);
        return record(quantity, quoteSupporter(quantity));
    }
}
`) };
      }
      return file;
    };
    const { invoke, booking } = session(backend, 128123 + index * 10, edit);
    assert.equal(invoke('quoteSupporter(uint256)', [2], guest), '50');
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });
    assert.equal(invoke('remaining()'), '12');

    assert.equal(invoke('reserveSupporter(uint256)', [2], guest), '50');
    assert.deepEqual(booking(guest), { seats: '2', quote: '50' });
    assert.equal(invoke('remaining()'), '10');
    assert.equal(invoke('cancel()', [], guest), '50');
    assert.deepEqual(booking(guest), { seats: '0', quote: '0' });
    assert.equal(invoke('remaining()'), '12');
  });
}
