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
    assert(actions.size()==6);
    assert(std::string(MainMenu[0].label)=="任务进度");
    assert(std::string(MainMenu[1].label)=="记忆");
    assert(std::string(MainMenu[2].label)=="配置");
    assert(std::string(MainMenu[3].label)=="帮助");
    assert(std::string(MainMenu[4].label)=="插件");
    assert(MainMenu[MemoryMenuIndex].action==MenuAction::Memory);
    assert(MainMenu[5].action==MenuAction::Quit);
    assert(MainMenu[HelpMenuIndex].action==MenuAction::Help);
    assert(MainMenu[TaskMenuIndex].action==MenuAction::Task);
    assert(HelpSections.size()==2);
    assert(std::string(HelpTexts[0]).find("Fn+空格")!=std::string::npos);
    assert(std::string(HelpTexts[1]).find("/clear")!=std::string::npos);
    assert(std::string(HelpTexts[1]).find("/compact")!=std::string::npos);
    for (auto status : {"执行命令", "等待命令", "整理任务", "处理文件", "调用工具"})
        assert(has_activity_dots(composer_activity(true,true,false,status)));
    assert(MainMenu[SettingsMenuIndex].action == MenuAction::Settings);
    assert(MainMenu[CatalogMenuIndex].action == MenuAction::Catalog);
    assert(CategoryPrefixes.size() == CategoryNames.size());
    assert(CategoryPrefixes[MemoryCategory] == "memory.");
    for (const auto& prefix : CategoryPrefixes) assert(prefix != "display.");
    for (const auto& prefix : CategoryPrefixes) assert(prefix != "speech.");
    for (const auto& item : SettingLabels.items()) assert(item.key().find("speech.") != 0);
}
