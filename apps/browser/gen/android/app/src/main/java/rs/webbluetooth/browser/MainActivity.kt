package rs.webbluetooth.browser

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

/**
 * Asks for the Bluetooth permissions before the web layer needs them.
 *
 * Declaring them in the manifest is not enough on Android 6 and later: they
 * are runtime permissions, and `webbluetooth-android` deliberately does not
 * request them — a library has no Activity to ask from and no business
 * deciding when a person is asked. So it happens here, once, at launch.
 *
 * A refusal is not treated as fatal. `getAvailability()` already has an
 * answer for "the radio is not usable", and a blank screen would be a worse
 * way to say it.
 */
class MainActivity : TauriActivity() {
  private val bluetoothPermissions: Array<String>
    get() = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
      // API 31 split scanning and connecting out of the location permission.
      arrayOf(Manifest.permission.BLUETOOTH_SCAN, Manifest.permission.BLUETOOTH_CONNECT)
    } else {
      // Before that, a BLE scan is treated as a way of locating the user.
      arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    val missing = bluetoothPermissions.filter {
      checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED
    }
    if (missing.isNotEmpty()) {
      requestPermissions(missing.toTypedArray(), BLUETOOTH_PERMISSION_REQUEST)
    }
  }

  private companion object {
    const val BLUETOOTH_PERMISSION_REQUEST = 0x0B7E
  }
}
