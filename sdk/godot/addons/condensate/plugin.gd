@tool
extends EditorPlugin
## Adds the `Condensate` autoload and the Project Settings > Condensate > Game Id setting.

const SETTING := "condensate/game_id"


func _enter_tree() -> void:
	if not ProjectSettings.has_setting(SETTING):
		ProjectSettings.set_setting(SETTING, "")
	ProjectSettings.set_initial_value(SETTING, "")
	ProjectSettings.set_as_basic(SETTING, true)
	ProjectSettings.add_property_info({
		"name": SETTING,
		"type": TYPE_STRING,
		"hint": PROPERTY_HINT_PLACEHOLDER_TEXT,
		"hint_string": "the game's id in Condensate, e.g. my-game",
	})


func _enable_plugin() -> void:
	add_autoload_singleton("Condensate", "res://addons/condensate/condensate.gd")


func _disable_plugin() -> void:
	remove_autoload_singleton("Condensate")
