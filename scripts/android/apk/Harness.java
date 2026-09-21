package io.webbluetooth.harness;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;

/** Host for the native harness: this class exists to hold a Context and the
 *  runtime permissions, which is the whole reason an APK is needed.
 *
 *  Everything else happens in Rust. The role comes from the launch intent so
 *  one APK can be either end of a conversation:
 *
 *      adb shell am start -n io.webbluetooth.harness/.Harness --es role peripheral
 */
public class Harness extends Activity {
    static final String TAG = "wbharness";

    static {
        System.loadLibrary("android_harness");
    }

    /** Runs the role and returns a report. Blocks, so never on the main thread. */
    private static native String nativeRun(Object context, String role);

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        final String role = getIntent().getStringExtra("role") != null
                ? getIntent().getStringExtra("role")
                : "central";

        // Asking is asynchronous, so the work waits for the grant rather than
        // racing it. On an emulator these are granted by `adb shell pm grant`
        // before launch, in which case this returns immediately.
        String[] wanted = {
            "android.permission.BLUETOOTH_SCAN",
            "android.permission.BLUETOOTH_CONNECT",
            "android.permission.BLUETOOTH_ADVERTISE",
        };
        requestPermissions(wanted, 1);
        start(role);
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] perms, int[] granted) {
        for (int i = 0; i < perms.length; i++) {
            Log.i(TAG, "permission " + perms[i] + " = " + granted[i]);
        }
    }

    private void start(final String role) {
        final Object context = getApplicationContext();
        new Thread(new Runnable() {
            public void run() {
                Log.i(TAG, "=== harness start: " + role + " ===");
                try {
                    String report = nativeRun(context, role);
                    for (String line : report.split("\n")) {
                        Log.i(TAG, line);
                    }
                } catch (Throwable t) {
                    Log.e(TAG, "harness threw: " + t);
                }
                Log.i(TAG, "=== harness end ===");
            }
        }).start();
    }
}
