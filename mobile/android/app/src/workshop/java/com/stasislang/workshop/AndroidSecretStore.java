package com.stasislang.workshop;

import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;

import java.nio.charset.StandardCharsets;
import java.security.KeyStore;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

final class AndroidSecretStore {
    private static final String KEY_ALIAS = "stasis_workshop_credentials_v1";
    private static final String ENCRYPTED_PREFIX = "encrypted_v1_";
    private static final String PAYLOAD_SEPARATOR = ".";

    private AndroidSecretStore() {}

    static String readAndMigrate(SharedPreferences preferences, String key) throws Exception {
        String encrypted = preferences.getString(ENCRYPTED_PREFIX + key, "");
        if (!encrypted.isEmpty()) return decrypt(key, encrypted);

        String legacyPlaintext = preferences.getString(key, "");
        if (!legacyPlaintext.isEmpty()) write(preferences, key, legacyPlaintext);
        return legacyPlaintext;
    }

    static void write(SharedPreferences preferences, String key, String value) throws Exception {
        SharedPreferences.Editor editor = preferences.edit().remove(key);
        if (value == null || value.isEmpty()) {
            editor.remove(ENCRYPTED_PREFIX + key);
        } else {
            editor.putString(ENCRYPTED_PREFIX + key, encrypt(key, value));
        }
        if (!editor.commit()) throw new IllegalStateException("credential preferences commit failed");
    }

    private static String encrypt(String preferenceKey, String value) throws Exception {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, credentialKey());
        cipher.updateAAD(preferenceKey.getBytes(StandardCharsets.UTF_8));
        byte[] encrypted = cipher.doFinal(value.getBytes(StandardCharsets.UTF_8));
        return Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP)
                + PAYLOAD_SEPARATOR
                + Base64.encodeToString(encrypted, Base64.NO_WRAP);
    }

    private static String decrypt(String preferenceKey, String payload) throws Exception {
        int separator = payload.indexOf(PAYLOAD_SEPARATOR);
        if (separator <= 0 || separator == payload.length() - 1) {
            throw new IllegalStateException("credential payload is invalid");
        }
        byte[] iv = Base64.decode(payload.substring(0, separator), Base64.NO_WRAP);
        byte[] encrypted = Base64.decode(payload.substring(separator + 1), Base64.NO_WRAP);
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.DECRYPT_MODE, credentialKey(), new GCMParameterSpec(128, iv));
        cipher.updateAAD(preferenceKey.getBytes(StandardCharsets.UTF_8));
        return new String(cipher.doFinal(encrypted), StandardCharsets.UTF_8);
    }

    private static SecretKey credentialKey() throws Exception {
        KeyStore keyStore = KeyStore.getInstance("AndroidKeyStore");
        keyStore.load(null);
        if (!keyStore.containsAlias(KEY_ALIAS)) {
            KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new KeyGenParameterSpec.Builder(KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .build());
            generator.generateKey();
        }
        return (SecretKey)keyStore.getKey(KEY_ALIAS, null);
    }
}
