/** Encrypt provider credentials for every entry point, migrating existing desktop data in place. */
import { createCipheriv, createDecipheriv, randomBytes, type CipherGCM, type DecipherGCM } from 'node:crypto';
import { constants, fstatSync, openSync, closeSync, readFileSync, writeFileSync, type Stats } from 'node:fs';
import path from 'node:path';
import { Entry } from '@napi-rs/keyring';
import { RouterError } from '../common/schemas/contracts.ts';
import type { SecretCipher } from '../products/router/repository/settings/file.ts';

const marker: Buffer = Buffer.from('pr-vault-v1\0');

export function isLegacySecret(value: Buffer): boolean {
  return !value.subarray(0, marker.length).equals(marker);
}

export function createCredentialVault(directory: string): SecretCipher {
  let key: Buffer | undefined;

  function getKey(create: boolean): Buffer {
    if (key) return key;
    try {
      key = process.platform === 'linux' ? linuxVaultKey(directory, create) : keychainVaultKey(directory, create);
      if (key.length !== 32) throw new Error('Invalid vault key');
      return key;
    } catch {
      key = undefined;
      throw new RouterError('VAULT_UNAVAILABLE', 'The Provider Router credential vault is unavailable. Check that this account can access its vault.');
    }
  }

  return {
    encrypt(value: string): Buffer {
      const nonce: Buffer = randomBytes(12);
      const cipher: CipherGCM = createCipheriv('aes-256-gcm', getKey(true), nonce);
      return Buffer.concat([marker, nonce, cipher.update(value, 'utf8'), cipher.final(), cipher.getAuthTag()]);
    },
    decrypt(value: Buffer): string {
      if (isLegacySecret(value)) throw new RouterError('VAULT_MIGRATION_REQUIRED', 'Open the desktop app once to migrate existing provider credentials, or enter the key again.');
      try {
        const decipher: DecipherGCM = createDecipheriv('aes-256-gcm', getKey(false), value.subarray(marker.length, marker.length + 12));
        decipher.setAuthTag(value.subarray(-16));
        return Buffer.concat([decipher.update(value.subarray(marker.length + 12, -16)), decipher.final()]).toString('utf8');
      } catch (error: unknown) {
        if (error instanceof RouterError) throw error;
        throw new RouterError('VAULT_UNAVAILABLE', 'The saved provider key could not be unlocked. Check the credential vault before entering a replacement key.');
      }
    },
  };
}

/** Headless Linux uses an account-private wrapping key that survives reboot without a graphical session. */
function linuxVaultKey(directory: string, create: boolean): Buffer {
  const file: string = path.join(directory, 'vault.key');
  let fd: number;
  try {
    fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  } catch (error: unknown) {
    if (!create || !hasCode(error, 'ENOENT')) throw error;
    const value: Buffer = randomBytes(32);
    writeFileSync(file, value, { mode: 0o600, flag: 'wx', flush: true });
    return value;
  }

  try {
    const stat: Stats = fstatSync(fd);
    if (!stat.isFile() || stat.size !== 32 || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) throw new Error('Vault key permissions are unsafe');
    return readFileSync(fd);
  } finally {
    closeSync(fd);
  }
}

function keychainVaultKey(directory: string, create: boolean): Buffer {
  const entry: Entry = new Entry('Provider Router', directory);
  const stored: string | null = entry.getPassword();
  if (stored) return Buffer.from(stored, 'base64');
  if (!create) throw new Error('Vault key is missing');
  const value: Buffer = randomBytes(32);
  entry.setPassword(value.toString('base64'));
  return value;
}

function hasCode(error: unknown, code: string): boolean {
  return error instanceof Error && 'code' in error && error.code === code;
}
