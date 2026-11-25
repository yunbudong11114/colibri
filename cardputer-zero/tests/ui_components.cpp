#include "composer_activity.hpp"
#include "menu.hpp"
#include <cassert>
#include <set>
#include <string>

int main() {
    using namespace colibri_ui;
    assert(composer_activity(true, true, false, "正在转写") == ComposerActivity::Transcribing);
    assert(composer_activity(false, true, false, "正在转写") == ComposerActivity::Idle);
    assert(composer_activity(true, false, false, "正在转写") == ComposerActivity::Idle);
    assert(composer_activity(true, false, true, "") == ComposerActivity::Recording);
    assert(composer_activity(true, true, false, "思考中") == ComposerActivity::Thinking);
    assert(composer_activity(true, true, false, "压缩上下文") == ComposerActivity::Compacting);
    assert(composer_activity(true, false, false, "压缩上下文") == ComposerActivity::Idle);
    assert(has_activity_dots(ComposerActivity::Compacting));
    for (auto activity : {ComposerActivity::Thinking, ComposerActivity::Transcribing, ComposerActivity::Compacting}) {
        for (unsigned phase = 0; phase < 3; ++phase) {
            unsigned raised = 0;
            for (unsigned dot = 0; dot < 3; ++dot) {
                const auto state = activity_dot(activity, phase * 180, dot);
                assert(state.opacity == 255);
                assert(state.offset_y == (dot == phase ? -3 : 0));
                raised += state.offset_y == -3;
            }
            assert(raised == 1);
        }
    }
    std::set<MenuAction> actions;
    for (const auto& item : MainMenu) assert(actions.insert(item.action).second);
    assert(actions.count(MenuAction::Compact) && actions.count(MenuAction::Clear));
    assert(MainMenu[SettingsMenuIndex].action == MenuAction::Settings);
    assert(MainMenu[CatalogMenuIndex].action == MenuAction::Catalog);
    assert(CategoryPrefixes.size() == CategoryNames.size());
    assert(CategoryPrefixes[MemoryCategory] == "memory.");
    assert(CategoryPrefixes[DisplayCategory] == "display.");
    for (const auto& prefix : CategoryPrefixes) assert(prefix != "speech.");
    for (const auto& item : SettingLabels.items()) assert(item.key().find("speech.") != 0);
}
