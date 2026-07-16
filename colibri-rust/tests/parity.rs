use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Debug)]
struct ParityEntry {
    python_file: &'static str,
    rust_tests: &'static [&'static str],
    status: &'static str,
}

fn parity_coverage_map() -> Vec<ParityEntry> {
    vec![
        ParityEntry {
            python_file: "test_channels.py",
            rust_tests: &[
                "auth_weixin_uses_native_http_and_saves_token_without_printing_secret",
                "gateway_session_cache_reuses_and_evicts_oldest_like_python",
                "gateway_session_transcript_injects_channel_metadata_like_python",
                "channel_permission_waiters_isolate_same_sender_across_channels",
                "gateway_channel_adapter_rejects_mismatched_envelope_like_python",
                "gateway_channel_registry_accepts_fake_adapter_without_gateway_changes",
                "gateway_channel_registry_rejects_duplicate_adapter_names",
                "inbound_router_idle_waits_for_active_release_like_python",
                "rust_channel_registry_builds_only_configured_enabled_adapters",
                "weixin_gateway_channel_exposes_transport_through_generic_adapter",
                "gateway_stop_refuses_unverified_pid_like_python",
                "weixin_updates_parse_text_and_media_with_context_like_python",
                "weixin_updates_keep_text_when_media_download_fails_like_python",
                "weixin_aes_ecb_pkcs7_round_trip_matches_python",
                "weixin_media_cleanup_removes_oldest_files_to_fit_budget_like_python",
                "weixin_permission_numeric_choices_match_python",
                "weixin_download_inbound_media_decrypts_and_stores_file_like_python",
                "weixin_send_media_encrypts_uploads_and_sends_metadata_like_python",
                "weixin_send_text_uses_unique_client_id_like_python",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_cli.py",
            rust_tests: &[
                "python_rust_cli_ask_output_matches_with_status_enabled",
                "python_rust_cli_diagnostics_output_matches",
                "python_rust_gateway_usage_output_matches",
                "repl_line_editor_backspace_removes_cjk_and_redraws_like_python",
                "repl_line_editor_history_navigation_does_not_print_escape_text_like_python",
                "repl_write_raw_tty_newline_returns_cursor_to_column_zero_like_python",
                "read_repl_line_reads_unicode_from_plain_stream_like_python",
                "handle_escape_sequence_navigates_history_for_arrow_keys_like_python",
                "read_escape_sequence_consumes_arrow_key_bytes_like_python",
                "repl_keeps_history_across_turns_for_arrow_navigation",
                "steering_pump_forwards_line_to_steer",
                "steering_pump_skips_read_while_permission_pending",
                "steering_pump_notifies_permission_pending_once",
                "try_read_line_returns_none_when_stdin_not_tty",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_config.py",
            rust_tests: &[
                "default_config_matches_python_runtime_defaults",
                "load_config_overrides_nested_values",
                "load_without_path_reads_user_default_config",
                "load_without_path_falls_back_when_user_default_missing",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_console.py",
            rust_tests: &[
                "python_rust_cli_ask_output_matches_with_status_enabled",
                "console_plain_answer_and_status_events_match_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_context.py",
            rust_tests: &[
                "session_records_fake_response",
                "session_writes_transcript_events",
                "session_uses_model_assisted_compact_and_retains_latest_user_like_python",
                "session_compacts_when_model_input_tokens_reach_threshold_like_python",
                "retain_recent_message_groups_keeps_tool_pairs_intact_like_python",
                "session_does_not_log_context_budget_for_token_triggered_compaction_like_python",
                "session_keeps_large_tool_result_text_for_model_context_like_python",
                "context_pressure_warning_is_not_injected_for_large_model_input_like_python",
                "session_falls_back_and_logs_compact_error_like_python",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_diagnostics.py",
            rust_tests: &["python_rust_cli_diagnostics_output_matches"],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_gateway_process.py",
            rust_tests: &[
                "gateway_status_reports_stale_state_file",
                "gateway_stop_refuses_unverified_pid_like_python",
                "python_rust_gateway_usage_output_matches",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_gateway_steering.py",
            rust_tests: &[
                "gateway_get_existing_does_not_create_session",
                "gateway_try_steer_enqueues_when_turn_active",
                "gateway_try_steer_works_while_session_taken_for_submit",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_inbound_router.py",
            rust_tests: &[
                "inbound_router_global_bound_and_orders_sessions_like_python",
                "inbound_router_same_session_not_concurrent_like_python",
                "inbound_router_idle_waits_for_active_release_like_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_channel_permission.py",
            rust_tests: &["weixin_permission_numeric_choices_match_python"],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_memory.py",
            rust_tests: &[
                "memory_context_bootstraps_and_loads_always_on_files",
                "memory_context_disabled_returns_empty_and_does_not_bootstrap",
                "memory_bootstrap_content_and_per_file_limits_match_python",
                "memory_write_rejects_traversal_like_python",
                "memory_write_supports_topic_and_python_result_text",
                "memory_context_replaces_invalid_utf8_like_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_model_factory.py",
            rust_tests: &[
                "default_config_matches_python_runtime_defaults",
                "model_factory_rejects_unknown_provider",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_openai_compatible_model.py",
            rust_tests: &[
                "openai_compatible_serializes_and_parses_tool_calls",
                "openai_compatible_requires_api_key_or_environment",
                "openai_tool_arguments_preserve_json_value_types_like_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_permissions.py",
            rust_tests: &[
                "permission_policy_matches_python_session_and_user_grants",
                "permission_policy_classifies_shell_redirection_as_file_path",
                "permission_policy_keeps_descriptor_and_null_redirections_as_shell",
                "permission_policy_keeps_inline_file_redirection_as_file_path",
                "permission_policy_hard_deny_wins_over_shell_redirection_file_path",
                "permission_policy_classifies_out_of_root_file_paths",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_permissions_store.py",
            rust_tests: &[
                "permission_policy_matches_python_session_and_user_grants",
                "user_permission_store_saves_and_loads_deduplicated_toml",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_session.py",
            rust_tests: &[
                "session_records_fake_response",
                "session_compacts_at_model_boundary_not_after_assistant_like_python",
                "session_close_closes_owned_transcript",
                "gateway_session_cache_close_closes_like_python",
                "submit_appends_media_paths_to_user_message_like_python",
                "session_sends_media_result_through_media_sender_like_python",
                "session_turns_media_sender_failure_into_tool_error_like_python",
                "session_denies_tool_calls_when_permission_mode_is_deny",
                "session_allows_read_only_tool_calls_by_default",
                "session_denies_write_and_execute_tools_without_approval_by_default",
                "session_writes_transcript_events",
                "session_uses_model_assisted_compact_and_retains_latest_user_like_python",
                "session_compacts_when_model_input_tokens_reach_threshold_like_python",
                "session_does_not_log_context_budget_for_token_triggered_compaction_like_python",
                "session_keeps_large_tool_result_text_for_model_context_like_python",
                "context_pressure_warning_is_not_injected_for_large_model_input_like_python",
                "session_falls_back_and_logs_compact_error_like_python",
                "session_round_limit_text_matches_python",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_steering.py",
            rust_tests: &[
                "steering_skip_result_constant_matches_python",
                "format_steering_ack_matches_python",
                "steer_rejected_when_turn_inactive",
                "steer_rejected_while_permission_pending",
                "steer_skips_remaining_tools_and_injects_user_message",
                "steer_during_text_only_complete_is_applied",
                "steering_queue_empty_after_normal_submit",
                "console_plain_answer_and_status_events_match_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_session_history.py",
            rust_tests: &[
                "transcript_history_loader_restores_complete_turns_and_strips_attachment_paths_like_python",
                "transcript_history_loader_pairs_each_source_by_completion_order_like_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_skills.py",
            rust_tests: &[
                "skill_catalog_includes_builtin_and_local_like_python",
                "skill_index_parses_metadata_and_builds_catalog_like_python",
                "skill_read_returns_bounded_body_like_python",
                "skill_run_executes_configured_command",
                "skill_yaml_frontmatter_parses_multiline_description_like_python",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_terminal_qr.py",
            rust_tests: &[
                "terminal_qr_outputs_block_qr_for_weixin_payload",
                "terminal_qr_returns_none_for_large_payload",
            ],
            status: "covered",
        },
        ParityEntry {
            python_file: "test_tools.py",
            rust_tests: &[
                "files_tool_lists_reads_and_writes_inside_allowed_root",
                "files_read_range_and_max_chars_match_python",
                "files_send_returns_media_result_for_allowed_file_like_python",
                "files_send_requires_channel_media_sender_like_python",
                "image_understand_uses_fake_vision_model_for_allowed_image_like_python",
                "shell_tool_reports_invalid_quoted_command_like_python",
                "shell_tool_executes_compound_command_with_real_shell",
                "shell_tool_rejects_denied_executable_in_compound_command",
                "shell_tool_rejects_denied_executable_after_background_operator",
                "shell_timeout_terminates_without_waiting_for_natural_exit_like_python",
                "shell_timeout_kills_background_process_group_like_python",
                "nonzero_shell_exit_uses_python_error_type",
                "web_search_requires_configured_baidu_api_key",
                "web_search_rejects_invalid_freshness_before_network_like_python",
                "web_search_posts_baidu_request_and_formats_references",
                "tool_schemas_match_python_property_types_and_required_fields",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_transcript.py",
            rust_tests: &[
                "session_writes_transcript_events",
                "gateway_session_transcript_injects_channel_metadata_like_python",
                "transcript_writer_removes_expired_files_but_preserves_active_like_python",
                "transcript_writer_removes_oldest_inactive_files_to_fit_size_limit_like_python",
            ],
            status: "partial",
        },
        ParityEntry {
            python_file: "test_vision.py",
            rust_tests: &[
                "default_config_matches_python_runtime_defaults",
                "image_understand_uses_fake_vision_model_for_allowed_image_like_python",
            ],
            status: "covered",
        },
    ]
}

fn mapped_python_tests_for_file(file: &str) -> &'static [&'static str] {
    match file {
        "test_channels.py" => &[
            "test_weixin_channel_parses_allowed_finished_text_message",
            "test_weixin_channel_run_dispatches_text_and_image_separately_from_same_poll",
            "test_weixin_channel_run_dispatches_text_and_image_separately_across_polls",
            "test_weixin_channel_run_keeps_two_text_messages_separate",
            "test_weixin_channel_routes_permission_reply_to_active_waiter",
            "test_weixin_channel_worker_failure_does_not_deadlock_when_queue_is_full",
            "test_weixin_channel_ignores_disallowed_sender",
            "test_weixin_channel_sends_text_with_context_token",
            "test_weixin_channel_sends_media_with_context_token",
            "test_weixin_channel_parses_inbound_file_media_message",
            "test_weixin_channel_parses_inbound_image_media_message",
            "test_weixin_channel_keeps_text_when_inbound_media_download_fails",
            "test_weixin_api_download_inbound_file_decrypts_and_stores_media",
            "test_cleanup_media_directory_removes_expired_files",
            "test_cleanup_media_directory_removes_oldest_files_to_fit_budget",
            "test_cleanup_media_directory_ignores_delete_errors",
            "test_write_inbound_media_reserves_space_for_new_file",
            "test_weixin_api_upload_media_encrypts_and_uploads_file",
            "test_weixin_api_send_media_uses_uploaded_metadata",
            "test_weixin_permission_prompter_sends_prompt_and_maps_reply",
            "test_weixin_permission_waiter_uses_full_channel_session_key",
            "test_weixin_permission_prompt_uses_absolute_file_path_and_summarizes_content",
            "test_gateway_session_cache_reuses_and_evicts_oldest",
            "test_gateway_session_cache_passes_shared_history_loader_to_new_sessions",
            "test_gateway_runner_handles_message_with_weixin_permission_policy",
            "test_gateway_runner_passes_inbound_media_paths_to_session",
            "test_gateway_runner_sends_file_tool_media_through_channel",
            "test_gateway_runner_writes_channel_metadata_to_transcript",
            "test_gateway_runner_runs_all_enabled_channels",
            "test_gateway_runner_waits_for_active_turn_before_closing_sessions",
            "test_weixin_standalone_run_waits_for_active_handler",
            "test_gateway_runner_rejects_duplicate_channel_names",
            "test_gateway_runner_rejects_envelope_from_wrong_channel",
            "test_perform_weixin_auth_prints_terminal_qr",
        ],
        "test_cli.py" => &[
            "test_ask_prints_fake_response",
            "test_ask_prints_status_to_stderr",
            "test_ask_can_disable_status",
            "test_ask_closes_session_transcript",
            "test_repl_exits_on_quit",
            "test_repl_exits_on_idle_timeout",
            "test_repl_idle_timeout_is_disabled_by_default",
            "test_gateway_without_action_prints_usage",
            "test_parser_accepts_gateway_run_command",
            "test_gateway_start_uses_process_manager",
            "test_gateway_status_uses_process_manager",
            "test_gateway_status_does_not_load_config",
            "test_gateway_stop_does_not_load_config",
            "test_auth_weixin_saves_token_without_printing_secret",
            "test_read_repl_line_reads_unicode_from_plain_stream",
            "test_repl_line_editor_backspace_removes_cjk_and_redraws_line",
            "test_repl_line_editor_up_and_down_navigate_history_without_printing_escape_text",
            "test_read_tty_byte_uses_unbuffered_fd_read",
            "test_read_escape_sequence_consumes_arrow_key_bytes",
            "test_write_raw_tty_newline_returns_cursor_to_column_zero",
            "test_diagnostics_prints_key_value_lines",
            "test_main_returns_one_for_expected_model_errors",
            "test_steering_pump_forwards_line_to_steer",
            "test_steering_pump_skips_read_while_permission_pending",
            "test_steering_pump_notifies_permission_pending_once",
            "test_try_read_line_returns_none_when_stdin_not_selectable",
            "test_try_read_line_abort_after_select_prevents_readline",
        ],
        "test_config.py" => &[
            "test_default_config_uses_small_device_limits",
            "test_load_config_overrides_nested_values",
            "test_load_without_path_reads_user_default_config",
            "test_load_without_path_falls_back_when_user_default_missing",
            "test_explicit_config_path_overrides_user_default",
            "test_legacy_model_input_char_limit_is_rejected",
            "test_legacy_model_input_byte_limit_is_rejected",
            "test_load_config_overrides_memory_values",
            "test_unknown_top_level_section_is_rejected",
            "test_deprecated_max_recall_topics_is_rejected",
            "test_unknown_nested_field_is_rejected",
            "test_expand_user_path_expands_home",
        ],
        "test_console.py" => &[
            "test_status_writer_prints_plain_prefixed_lines",
            "test_status_writer_can_be_disabled",
            "test_status_transcript_maps_selected_events",
            "test_status_transcript_emits_steered_line",
            "test_format_plain_answer_strips_markdown_and_flattens_tables",
        ],
        "test_context.py" => &[
            "test_summarize_user_and_assistant_messages",
            "test_summarize_tool_message_uses_metadata_not_full_output",
            "test_append_summary_keeps_tail_within_limit",
            "test_format_model_summary_strips_analysis_and_keeps_summary_body",
        ],
        "test_diagnostics.py" => &[
            "test_build_diagnostics_reports_core_fields",
            "test_diagnostics_reports_user_permissions_file",
            "test_rss_kb_returns_integer_or_none",
        ],
        "test_gateway_process.py" => &[
            "test_gateway_process_start_writes_state_and_uses_gateway_run",
            "test_gateway_process_status_handles_missing_state",
            "test_format_gateway_status_is_key_value",
            "test_gateway_process_stop_refuses_unverified_pid",
            "test_gateway_process_stop_terminates_verified_gateway_pid",
        ],
        "test_gateway_steering.py" => &[
            "test_try_steer_returns_false_when_no_session",
            "test_try_steer_enqueues_when_turn_active",
            "test_get_existing_does_not_create_session",
            "test_take_or_create_keeps_steer_available_while_session_outside_cache",
            "test_weixin_receive_skips_queue_when_try_steer_true",
            "test_worker_skips_send_on_empty_reply",
            "test_handle_message_steers_when_turn_active",
        ],
        "test_inbound_router.py" => &[
            "test_inbound_router_bounds_global_pending_and_orders_per_session",
            "test_inbound_router_same_session_not_concurrent",
            "test_inbound_router_is_idle_only_after_active_turn_releases",
        ],
        "test_channel_permission.py" => &[
            "test_channel_text_permission_prompter_is_transport_agnostic",
            "test_format_channel_permission_prompt_includes_choices",
        ],
        "test_memory.py" => &[
            "test_context_loads_memory_and_user_files",
            "test_context_bootstraps_sample_files_when_memory_root_is_missing",
            "test_context_bootstraps_sample_files_when_memory_root_has_no_files",
            "test_context_bootstrap_does_not_overwrite_existing_memory_files",
            "test_context_does_not_inject_index_or_topics",
            "test_context_obeys_character_budget",
            "test_context_truncates_without_write_guidance_when_always_on_files_exceed_file_limits",
            "test_context_disabled_returns_empty_result",
            "test_context_disabled_does_not_bootstrap_missing_files",
        ],
        "test_model_factory.py" => &[
            "test_factory_returns_fake_model_for_default_provider",
            "test_factory_returns_openai_compatible_model",
            "test_factory_rejects_unknown_provider",
        ],
        "test_openai_compatible_model.py" => &[
            "test_from_config_requires_api_key",
            "test_from_config_prefers_config_api_key",
            "test_from_config_falls_back_to_colibri_api_key",
            "test_complete_builds_chat_completion_request",
            "test_complete_passes_tools_when_present",
            "test_complete_serializes_tool_result_messages",
            "test_complete_serializes_assistant_tool_calls",
            "test_complete_parses_tool_calls",
            "test_complete_rejects_empty_choices",
            "test_request_json_turns_http_error_into_model_error",
            "test_request_json_preserves_chinese_utf8",
        ],
        "test_permissions_store.py" => &[
            "test_user_permission_store_loads_missing_file_as_empty",
            "test_user_permission_store_saves_and_loads_deduplicated_toml",
            "test_user_permission_store_loads_file_roots",
            "test_user_permission_store_loads_shell_executables",
            "test_user_permission_store_ignores_obsolete_shell_prefixes",
            "test_user_permission_store_reuses_cached_parse_when_file_is_unchanged",
            "test_user_permission_store_refreshes_cache_after_atomic_replacement",
            "test_user_permission_store_merge_preserves_concurrent_stale_grants",
        ],
        "test_permissions.py" => &[
            "test_read_only_tool_is_allowed_under_default_policy",
            "test_confirm_policy_calls_prompter",
            "test_numeric_session_choice_allows_tool_for_current_session",
            "test_concurrent_user_grants_merge_after_prompt_interleaving",
            "test_deny_policy_blocks_tool_without_prompting",
            "test_allow_read_confirm_write_confirms_non_read_only_tool",
            "test_shell_command_prompts_when_no_grant",
            "test_shell_session_command_grant_allows_second_call_without_prompt",
            "test_shell_session_executable_grant_allows_same_executable",
            "test_shell_user_executable_choice_persists_executable",
            "test_shell_user_command_grant_is_exact",
            "test_shell_numeric_user_command_choice_persists_exact_command",
            "test_shell_user_executable_grant_matches_executable_like_executable_session",
            "test_shell_hard_deny_blocks_without_prompt",
            "test_shell_hard_deny_wins_over_redirection_file_path_prompt",
            "test_out_of_root_file_path_prompts_instead_of_default_allow",
            "test_out_of_root_image_path_prompts_instead_of_default_allow",
            "test_out_of_root_files_write_prompts_as_file_path",
            "test_in_root_files_write_prompts_with_absolute_path_and_content_summary",
            "test_memory_write_prompt_summarizes_content_without_absolute_path",
            "test_shell_redirection_to_out_of_root_path_prompts_as_file_path",
            "test_shell_descriptor_and_null_redirections_keep_shell_permissions",
            "test_shell_inline_file_redirection_still_prompts_as_file_path",
            "test_files_under_startup_cwd_are_allowed_without_prompt",
            "test_file_path_session_grant_allows_same_resolved_path_without_prompt",
            "test_file_path_session_grant_allows_children_under_same_directory",
            "test_file_path_user_root_grant_allows_children_without_prompt",
        ],
        "test_session_history.py" => &[
            "test_loader_restores_only_complete_final_turns_and_strips_attachment_paths",
            "test_loader_pairs_each_source_then_merges_turns_by_completion_order",
            "test_loader_applies_message_and_character_limits_to_whole_turns",
            "test_loader_reads_newest_file_tails_within_scan_budget",
            "test_default_loader_uses_colibri_home_and_session_config",
        ],
        "test_session.py" => &[
            "test_submit_records_user_and_assistant_messages",
            "test_submit_restores_history_once_before_new_user_message",
            "test_reset_does_not_restore_old_transcript_again",
            "test_session_reuses_lazy_runtime_dependencies_across_submits",
            "test_history_restore_error_is_logged_and_does_not_block_submit",
            "test_submit_appends_media_paths_to_user_message",
            "test_system_prompt_has_sentence_spacing",
            "test_session_keeps_only_recent_messages",
            "test_session_compacts_at_model_boundary_not_after_assistant",
            "test_session_compacts_message_buffer_into_summary",
            "test_session_does_not_compact_before_trigger_message_limit",
            "test_session_retains_latest_user_message_even_outside_recent_window",
            "test_session_recent_limit_keeps_complete_tool_call_group",
            "test_session_recent_limit_keeps_tool_group_whole_when_group_exceeds_limit",
            "test_session_uses_model_assisted_compact_without_tools",
            "test_session_falls_back_when_model_assisted_compact_fails",
            "test_session_summary_is_injected_without_persisting_it",
            "test_session_logs_context_compact_event",
            "test_session_compacts_when_model_input_tokens_reach_threshold",
            "test_tool_result_context_keeps_large_success_text_for_model",
            "test_context_budget_event_is_not_written_for_token_triggered_compaction",
            "test_context_pressure_warning_is_not_injected_for_large_model_input",
            "test_reset_clears_messages_and_summary",
            "test_session_sends_media_result_through_media_sender",
            "test_session_turns_media_sender_failure_into_tool_error",
            "test_submit_executes_tool_call_and_returns_final_text",
            "test_submit_stops_at_max_tool_rounds",
            "test_denied_tool_call_adds_result_without_running_tool",
            "test_session_returns_user_denial_to_model",
            "test_session_allows_out_of_root_file_path_after_dynamic_permission",
            "test_session_file_directory_grant_passes_root_to_file_tool",
            "test_session_writes_transcript_events",
            "test_session_logs_dynamic_permission_payload",
            "test_session_writes_round_limit_event",
            "test_close_closes_transcript",
            "test_memory_write_uses_permission_confirmation",
            "test_skill_run_uses_permission_confirmation",
            "test_session_injects_always_on_memory_without_persisting_it",
            "test_session_logs_memory_context_event",
            "test_session_injects_skill_catalog_without_persisting_it",
        ],
        "test_steering.py" => &[
            "test_skip_result_constant",
            "test_ack_with_short_preview",
            "test_ack_truncates_preview_at_20_chars",
            "test_ack_omits_preview_when_empty",
            "test_steer_rejected_when_turn_inactive",
            "test_steer_rejected_while_permission_pending",
            "test_steer_skips_remaining_tools_and_injects_user_message",
            "test_steer_during_text_only_complete_is_applied",
            "test_steering_queue_empty_after_normal_submit",
        ],
        "test_skills.py" => &[
            "test_skill_index_scans_local_skills_without_storing_bodies",
            "test_skill_index_includes_builtin_create_colibri_skill_without_user_dir",
            "test_skill_catalog_includes_builtin_and_local_without_bodies",
            "test_skill_catalog_is_bounded",
            "test_skill_read_returns_bounded_body",
            "test_skill_read_rejects_unknown_name",
            "test_skill_index_parses_command_metadata",
            "test_skill_run_executes_declared_local_command",
            "test_skill_run_rejects_missing_command",
            "test_skill_index_skips_invalid_yaml_skill",
            "test_skill_read_lists_configured_commands",
            "test_builtin_creation_skill_uses_yaml_frontmatter",
            "test_skills_dirs_config_is_rejected",
            "test_skills_max_loaded_config_is_rejected",
        ],
        "test_terminal_qr.py" => &[
            "test_render_terminal_qr_outputs_block_qr_for_weixin_payload",
            "test_render_terminal_qr_returns_none_for_large_payload",
        ],
        "test_tools.py" => &[
            "test_registry_exposes_enabled_builtin_tool_specs",
            "test_registry_gets_registered_tool_by_name",
            "test_registry_rejects_unknown_tool",
            "test_files_list_lists_allowed_directory",
            "test_files_list_rejects_disallowed_directory",
            "test_files_read_reads_allowed_file_and_truncates",
            "test_files_read_reads_line_range_and_respects_max_chars",
            "test_files_read_rejects_invalid_line_range",
            "test_files_write_writes_allowed_file",
            "test_files_write_rejects_disallowed_file",
            "test_files_send_returns_media_result_for_allowed_file",
            "test_files_send_requires_channel_media_sender",
            "test_files_send_rejects_directory",
            "test_shell_run_executes_command_after_permission_phase",
            "test_shell_run_does_not_require_allowlist_after_permission_phase",
            "test_shell_run_rejects_denied_command",
            "test_shell_run_executes_compound_command_with_real_shell",
            "test_shell_run_rejects_denied_executable_in_compound_command",
            "test_shell_run_rejects_denied_executable_after_background_operator",
            "test_shell_run_times_out_slow_command",
            "test_shell_run_timeout_kills_background_process_group",
            "test_web_search_builds_baidu_request_and_formats_results",
            "test_web_search_requires_configured_baidu_api_key",
            "test_memory_list_returns_builtin_files_and_sorted_topic_names",
            "test_memory_read_reads_builtin_file_topic_shorthand_and_rejects_traversal",
            "test_memory_search_only_searches_index_lines_with_limit",
            "test_memory_search_does_not_scan_topic_content",
            "test_memory_write_appends_and_replaces_files",
            "test_memory_write_description_contains_function_targets_and_format_guidance",
            "test_memory_write_warns_when_short_memory_file_exceeds_limit",
            "test_memory_write_is_not_read_only",
            "test_skill_read_is_read_only",
            "test_skill_run_is_not_read_only",
        ],
        "test_transcript.py" => &[
            "test_transcript_writer_writes_jsonl_event",
            "test_default_transcript_path_uses_colibri_home",
            "test_scoped_transcript_writer_injects_metadata_without_closing_base",
            "test_transcript_writer_removes_expired_files_but_preserves_active_file",
            "test_transcript_writer_removes_oldest_inactive_files_to_fit_size_limit",
            "test_transcript_writer_throttles_cleanup_during_writes",
        ],
        "test_vision.py" => &[
            "test_vision_config_falls_back_to_agent_model",
            "test_image_tool_reads_allowed_image_without_permission_prompt",
            "test_image_tool_rejects_non_image_file",
            "test_image_analyzer_builds_bounded_data_url",
            "test_image_analyzer_rejects_oversized_image",
            "test_openai_compatible_image_request_uses_multimodal_content",
            "test_session_can_call_image_tool_after_media_path_is_received",
        ],
        _ => &[],
    }
}

#[test]
fn python_test_coverage_map_covers_all_unit_files() {
    let expected = [
        "test_channels.py",
        "test_cli.py",
        "test_config.py",
        "test_console.py",
        "test_context.py",
        "test_diagnostics.py",
        "test_gateway_process.py",
        "test_gateway_steering.py",
        "test_inbound_router.py",
        "test_channel_permission.py",
        "test_memory.py",
        "test_model_factory.py",
        "test_openai_compatible_model.py",
        "test_permissions.py",
        "test_permissions_store.py",
        "test_session.py",
        "test_session_history.py",
        "test_skills.py",
        "test_steering.py",
        "test_terminal_qr.py",
        "test_tools.py",
        "test_transcript.py",
        "test_vision.py",
    ];
    let mapped = parity_coverage_map();
    let rust_tests = rust_test_functions();
    for file in expected {
        let entry = mapped
            .iter()
            .find(|entry| entry.python_file == file)
            .unwrap_or_else(|| panic!("missing parity mapping for {file}"));
        assert!(
            !entry.rust_tests.is_empty(),
            "missing Rust tests for {}",
            entry.python_file
        );
        assert!(
            entry.status == "covered" || entry.status == "partial",
            "uncovered parity status for {}",
            entry.python_file
        );
        for rust_test in entry.rust_tests {
            assert!(
                rust_tests.contains(*rust_test),
                "mapped Rust test does not exist for {}: {}",
                entry.python_file,
                rust_test
            );
        }
        let actual_python_tests = python_test_functions(file);
        let mapped_python_tests = mapped_python_tests_for_file(file)
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual_python_tests, mapped_python_tests,
            "Python test function parity map mismatch for {}",
            file
        );
    }
}

#[test]
fn python_rust_cli_ask_output_matches_with_status_enabled() {
    let fixture = CliFixture::new("ask-status-enabled", true);

    let python = run_python_cli(&fixture, &["ask", "status"]);
    let rust = run_rust_cli(&fixture, &["ask", "status"]);

    assert_same_output("ask status", &python, &rust);
}

#[test]
fn python_rust_cli_diagnostics_output_matches() {
    let fixture = CliFixture::new("diagnostics", false);

    let python = run_python_cli(&fixture, &["diagnostics"]);
    let rust = run_rust_cli(&fixture, &["diagnostics"]);

    assert_same_output("diagnostics", &python, &rust);
}

#[test]
fn python_rust_gateway_usage_output_matches() {
    let fixture = CliFixture::new("gateway-usage", false);

    let python = run_python_cli(&fixture, &["gateway"]);
    let rust = run_rust_cli(&fixture, &["gateway"]);

    assert_same_output("gateway usage", &python, &rust);
}

struct CliFixture {
    home: PathBuf,
    config_path: PathBuf,
}

impl CliFixture {
    fn new(name: &str, status: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "colibri-rust-parity-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "[model]\nprovider = \"fake\"\nmodel = \"fake-colibri-model\"\n\n[console]\nstatus = {}\n",
                if status { "true" } else { "false" }
            ),
        )
        .unwrap();
        Self {
            home: root,
            config_path,
        }
    }
}

fn run_python_cli(fixture: &CliFixture, args: &[&str]) -> Output {
    let mut command = Command::new(uv_bin());
    command
        .arg("run")
        .arg("python")
        .arg("-m")
        .arg("colibri.cli")
        .arg("--config")
        .arg(&fixture.config_path)
        .args(args)
        .current_dir(repo_root())
        .env("HOME", &fixture.home);
    command.output().expect("run Python CLI")
}

fn run_rust_cli(fixture: &CliFixture, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_colibri"));
    command
        .arg("--config")
        .arg(&fixture.config_path)
        .args(args)
        .current_dir(repo_root())
        .env("HOME", &fixture.home);
    command.output().expect("run Rust CLI")
}

fn assert_same_output(label: &str, python: &Output, rust: &Output) {
    assert_eq!(
        python.status.code(),
        rust.status.code(),
        "{label}: exit code mismatch\npython stderr:\n{}\nrust stderr:\n{}",
        String::from_utf8_lossy(&python.stderr),
        String::from_utf8_lossy(&rust.stderr)
    );
    let python_stdout = normalize_cross_runtime_output(&String::from_utf8_lossy(&python.stdout));
    let rust_stdout = normalize_cross_runtime_output(&String::from_utf8_lossy(&rust.stdout));
    let python_stderr = normalize_cross_runtime_output(&String::from_utf8_lossy(&python.stderr));
    let rust_stderr = normalize_cross_runtime_output(&String::from_utf8_lossy(&rust.stderr));
    assert_eq!(python_stdout, rust_stdout, "{label}: stdout mismatch");
    assert_eq!(python_stderr, rust_stderr, "{label}: stderr mismatch");
}

fn python_test_functions(file: &str) -> BTreeSet<&'static str> {
    let path = repo_root().join("tests/unit").join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {}", path.display(), error));
    text.lines()
        .filter_map(|line| line.strip_prefix("def test_"))
        .map(|rest| {
            let name = rest.split_once('(').map(|(name, _)| name).unwrap_or(rest);
            Box::leak(format!("test_{}", name).into_boxed_str()) as &'static str
        })
        .collect()
}

fn rust_test_functions() -> BTreeSet<&'static str> {
    let tests_dir = repo_root().join("colibri-rust/tests");
    let mut names = BTreeSet::new();
    for file in ["parity.rs", "runtime.rs"] {
        let path = tests_dir.join(file);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {}", path.display(), error));
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("fn ") else {
                continue;
            };
            let name = rest.split_once('(').map(|(name, _)| name).unwrap_or(rest);
            names.insert(Box::leak(name.to_string().into_boxed_str()) as &'static str);
        }
    }
    names
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn uv_bin() -> String {
    std::env::var("UV_BIN").unwrap_or_else(|_| "uv".to_string())
}

fn normalize_cross_runtime_output(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with("python=") || line.starts_with("rust=") {
                "runtime=<normalized> platform=<normalized>".to_string()
            } else if let Some((prefix, _rss)) = line.split_once(" rss_kb=") {
                format!("{prefix} rss_kb=<normalized>")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + if text.ends_with('\n') { "\n" } else { "" }
}
