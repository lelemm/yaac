package dev.yaac.android.core

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build

/**
 * The phone's audio during a call: communication mode (so the platform's echo canceller runs
 * and the call's volume is the call volume), audio focus, and the output, chosen by
 * [CallRoute] between the speaker, the earpiece and a headset. [release] puts everything back.
 */
class CallAudio(context: Context) {
    private val am = context.getSystemService(AudioManager::class.java)
    private var savedMode = AudioManager.MODE_NORMAL
    private var focus: AudioFocusRequest? = null
    private var held = false

    fun acquire() {
        if (held) return
        held = true
        savedMode = am.mode
        val attributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
        focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(attributes)
            .setOnAudioFocusChangeListener { }
            .build()
            .also { am.requestAudioFocus(it) }
        am.mode = AudioManager.MODE_IN_COMMUNICATION
    }

    /** The outputs a call can use now. */
    fun available(): List<CallOutput> = devices().keys.toList()

    /** Routes the call to the speaker, or off it to a headset or the earpiece; returns where it went. */
    @Suppress("DEPRECATION")
    fun route(speaker: Boolean): CallOutput? {
        val devices = devices()
        val pick = CallRoute.pick(devices.keys.toList(), speaker) ?: return null
        if (Build.VERSION.SDK_INT >= 31) {
            devices[pick]?.let { am.setCommunicationDevice(it) }
        } else {
            am.isSpeakerphoneOn = pick == CallOutput.Speaker
            if (pick == CallOutput.Bluetooth) { am.startBluetoothSco(); am.isBluetoothScoOn = true }
            else if (am.isBluetoothScoOn) { am.isBluetoothScoOn = false; am.stopBluetoothSco() }
        }
        return pick
    }

    @Suppress("DEPRECATION")
    fun release() {
        if (!held) return
        held = false
        if (Build.VERSION.SDK_INT >= 31) am.clearCommunicationDevice()
        else {
            am.isSpeakerphoneOn = false
            if (am.isBluetoothScoOn) { am.isBluetoothScoOn = false; am.stopBluetoothSco() }
        }
        am.mode = savedMode
        focus?.let { am.abandonAudioFocusRequest(it) }
        focus = null
    }

    private fun devices(): Map<CallOutput, AudioDeviceInfo?> {
        val list: List<AudioDeviceInfo> = if (Build.VERSION.SDK_INT >= 31) am.availableCommunicationDevices
            else am.getDevices(AudioManager.GET_DEVICES_OUTPUTS).toList()
        val out = LinkedHashMap<CallOutput, AudioDeviceInfo?>()
        for (d in list) {
            val kind = when (d.type) {
                AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> CallOutput.Earpiece
                AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> CallOutput.Speaker
                AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_WIRED_HEADPHONES -> CallOutput.Wired
                AudioDeviceInfo.TYPE_USB_HEADSET -> CallOutput.Usb
                AudioDeviceInfo.TYPE_BLUETOOTH_SCO -> CallOutput.Bluetooth
                else -> if (Build.VERSION.SDK_INT >= 31 && d.type == AudioDeviceInfo.TYPE_BLE_HEADSET) CallOutput.Bluetooth else null
            } ?: continue
            out.putIfAbsent(kind, d)
        }
        return out
    }
}
