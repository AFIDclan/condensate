@tool
extends EditorPlugin
## Adds the `Condensate` autoload and its Project Settings: Condensate > Game Id, and
## Condensate > Feedback Screenshot Key.

const SETTING := "condensate/game_id"
const KEY_SETTING := "condensate/feedback_screenshot_key"


func _enter_tree() -> void:
	_add_setting(SETTING, "", "the game's id in Condensate, e.g. my-game")
	_add_setting(KEY_SETTING, "F12", "key players press to take a feedback screenshot")


func _add_setting(name: String, value: String, hint: String) -> void:
	if not ProjectSettings.has_setting(name):
		ProjectSettings.set_setting(name, value)
	ProjectSettings.set_initial_value(name, value)
	ProjectSettings.set_as_basic(name, true)
	ProjectSettings.add_property_info({
		"name": name,
		"type": TYPE_STRING,
		"hint": PROPERTY_HINT_PLACEHOLDER_TEXT,
		"hint_string": hint,
	})


func _enable_plugin() -> void:
	add_autoload_singleton("Condensate", "res://addons/condensate/condensate.gd")


func _disable_plugin() -> void:
	remove_autoload_singleton("Condensate")
