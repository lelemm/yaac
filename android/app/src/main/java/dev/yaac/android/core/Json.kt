package dev.yaac.android.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

/** Lenient readers over view frames, with the same fallbacks the WPF renderer uses. */
val FrameJson = Json { ignoreUnknownKeys = true; isLenient = true }

operator fun JsonElement?.get(key: String): JsonElement? = (this as? JsonObject)?.get(key)

val JsonElement?.obj: JsonObject? get() = this as? JsonObject
val JsonElement?.list: List<JsonElement> get() = (this as? JsonArray) ?: emptyList()
val JsonElement?.objects: List<JsonObject> get() = list.mapNotNull { it as? JsonObject }

/** A non-empty string, or null (WPF's `OptionalText`). */
val JsonElement?.text: String?
    get() = (this as? JsonPrimitive)?.takeIf { it.isString }?.content?.takeIf { it.isNotEmpty() }

/** A string, or "" (WPF's `Text`). */
val JsonElement?.str: String get() = text ?: ""

/** A number rendered as text, or a non-empty string (WPF's `Scalar`). */
val JsonElement?.scalar: String?
    get() = (this as? JsonPrimitive)?.let { if (it.isString) it.content.takeIf { s -> s.isNotEmpty() } else if (it is JsonNull) null else it.content }

fun JsonElement?.flag(fallback: Boolean = false): Boolean =
    (this as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull ?: fallback

val JsonElement?.long: Long? get() = (this as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull
val JsonElement?.int: Int? get() = long?.toInt()
val JsonElement?.double: Double? get() = (this as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull

val JsonElement?.isNullish: Boolean get() = this == null || this is JsonNull

fun jsonOf(vararg pairs: Pair<String, Any?>): JsonObject = JsonObject(pairs.associate { (k, v) -> k to toJson(v) })

fun toJson(v: Any?): JsonElement = when (v) {
    null -> JsonNull
    is JsonElement -> v
    is String -> JsonPrimitive(v)
    is Number -> JsonPrimitive(v)
    is Boolean -> JsonPrimitive(v)
    is Map<*, *> -> JsonObject(v.entries.associate { (k, x) -> k.toString() to toJson(x) })
    is Iterable<*> -> JsonArray(v.map { toJson(it) })
    is Array<*> -> JsonArray(v.map { toJson(it) })
    else -> JsonPrimitive(v.toString())
}

@Suppress("unused")
private fun JsonPrimitive.stringOrNull(): String? = contentOrNull
