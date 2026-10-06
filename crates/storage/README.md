# storage：项目持久化

`storage` 为匿名受试者和 EEG 记录提供本地项目存储。它公开与数据库实现无关的仓储 trait，并提供基于 `redb` 与独立二进制样本文件的 `ProjectStorage` 实现。

当前只持久化 `domain::Subject` 和 `domain::EegRecording`。`Visit`、量表、特征、模型结果、综合评估和报告尚不在本 crate 的存储范围内。

## 引入依赖

工作区内的 crate 可以使用路径依赖：

```toml
[dependencies]
chrono = "0.4"
domain = { path = "../domain" }
storage = { path = "../storage" }
```

## 快速开始

先创建受试者，再创建属于该受试者的 EEG 记录。仓储方法由 `SubjectRepository` 和 `RecordingRepository` trait 提供，因此调用处需要将 trait 引入作用域。

```rust
use chrono::Utc;
use domain::{
    Channel, ChannelKind, EegRecording, RecordingId, RecordingMetadata,
    RecordingState, Sex, Subject, SubjectId,
};
use std::path::Path;
use storage::{ProjectStorage, RecordingRepository, SubjectRepository};

fn create_project(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let storage = ProjectStorage::open(project_dir)?;

    let subject = Subject::new(
        SubjectId::new(),
        Some(32),
        Sex::Female,
        Utc::now(),
    )?;
    storage.create_subject(&subject)?;

    let recording = EegRecording::new(
        RecordingId::new(),
        subject.id,
        250.0,
        vec![Channel::new("Fp1", ChannelKind::Eeg, "uV")?],
        vec![vec![0.1, 0.2, 0.3, 0.4]],
        RecordingState::Raw,
        RecordingMetadata::default(),
    )?;
    storage.create_recording(&recording)?;

    // 列表页只读取 metadata，不加载可能很大的样本矩阵。
    let summaries = storage.list_recording_metadata(subject.id)?;
    assert_eq!(summaries[0].sample_count, 4);

    // 算法需要样本时再恢复完整的领域对象。
    let restored = storage
        .get_recording(recording.id)?
        .expect("刚写入的记录应当存在");
    assert_eq!(restored.samples, recording.samples);
    Ok(())
}
```

`ProjectStorage::open` 接受项目目录路径。目录不存在时会创建目录、数据库和所需表；目录已经存在时会重新打开已有项目。`ProjectStorage` 可克隆，克隆值共享同一个数据库句柄。

## 仓储接口

### 受试者

`SubjectRepository` 提供：

```text
create_subject
get_subject
list_subjects
update_subject
delete_subject
```

`create` 和 `update` 有意分离：创建已存在的 ID 返回 `StorageError::AlreadyExists`，更新或删除不存在的 ID 返回 `StorageError::NotFound`，不会静默覆盖或忽略。

如果受试者仍有关联 EEG 记录，`delete_subject` 返回 `StorageError::SubjectHasRecordings`。调用方必须先删除这些记录：

```rust
use storage::{RecordingRepository, SubjectRepository};

# fn remove(storage: &storage::ProjectStorage, subject_id: domain::SubjectId)
#     -> storage::StorageResult<()> {
for metadata in storage.list_recording_metadata(subject_id)? {
    storage.delete_recording(metadata.id)?;
}
storage.delete_subject(subject_id)?;
# Ok(())
# }
```

### EEG 记录

`RecordingRepository` 提供：

```text
create_recording
get_recording
get_recording_metadata
list_recording_metadata
update_recording
delete_recording
```

记录引用的 `subject_id` 必须已经存在，否则创建或更新返回 `StorageError::MissingSubject`。`list_recording_metadata(subject_id)` 只返回指定受试者的记录，不读取样本文件。

`get_recording_metadata` 返回 `StoredRecordingMetadata`，其中包含 ID、采样率、通道、样本数、状态、时长、处理历史和采集元数据，但不包含样本矩阵。内部样本文件名是私有实现细节。

## 面向 trait 编程

应用服务可以只依赖仓储 trait，便于替换实现或在测试中注入替身：

```rust
use domain::Subject;
use storage::{StorageResult, SubjectRepository};

fn save_subject(repository: &dyn SubjectRepository, subject: &Subject) -> StorageResult<()> {
    repository.create_subject(subject)
}
```

两个仓储 trait 都要求 `Send + Sync`。`ProjectStorage` 的每次公共操作使用独立事务；`redb` 允许并发读取并串行化写入。

## 磁盘布局

一个项目目录的当前布局如下：

```text
project-directory/
├── project.redb
└── recordings/
    └── <recording-uuid>/
        └── samples-<payload-uuid>.bin
```

- `project.redb` 保存受试者 JSON 和 EEG metadata JSON；
- 样本矩阵单独保存在记录目录中，避免列表和 metadata 查询加载大块数据；
- 样本文件采用带 magic、通道数和每通道样本数的版本化小端 `f32` 格式；读取时会校验文件格式、形状、大小及领域约束；
- 更新 EEG 记录时先写入新样本文件，数据库提交新 metadata 后再清理旧文件；失败的新写入会尽力回滚其样本文件。

`DATABASE_FILE_NAME` 和 `RECORDINGS_DIRECTORY_NAME` 可用于测试或运维检查，但具体文件编码和文件名不是公共兼容协议。应用应始终通过仓储 API 访问数据，不应自行解析或改写项目目录。

## CRUD 与一致性语义

| 操作 | 行为 |
| --- | --- |
| 创建受试者 | ID 必须尚不存在 |
| 更新受试者 | ID 必须已经存在 |
| 删除受试者 | ID 必须存在，且不能仍有关联记录 |
| 创建记录 | 记录 ID 必须尚不存在，所属受试者必须存在 |
| 更新记录 | 记录 ID 与所属受试者都必须存在；样本和 metadata 一起替换 |
| 删除记录 | 先提交数据库删除，再删除该记录的样本目录 |
| 查询单个实体 | 不存在时返回 `Ok(None)`，而不是 `NotFound` |

每次数据库写入都使用事务，失败不会留下部分数据库行。仍需注意文件系统与数据库不是一个跨介质事务：例如删除记录时数据库提交成功、随后目录删除失败，会返回 `StorageError::Io`，但数据库中的记录已经删除。应用可记录错误并安排清理残留目录。

## 错误处理

所有接口统一返回 `StorageResult<T> = Result<T, StorageError>`。常用错误包括：

- `AlreadyExists`：创建时 ID 冲突；
- `NotFound`：更新或删除的实体不存在；
- `MissingSubject`：EEG 记录引用不存在的受试者；
- `SubjectHasRecordings`：尝试删除仍拥有记录的受试者；
- `Database`、`Serialization`、`Io`：数据库、JSON 或文件系统失败；
- `CorruptData`：样本文件格式、大小、形状或恢复后的领域对象无效。

可以按变体给 UI 返回明确提示：

```rust
use storage::{EntityKind, StorageError, SubjectRepository};

# fn create(
#     storage: &storage::ProjectStorage,
#     subject: &domain::Subject,
# ) -> storage::StorageResult<()> {
match storage.create_subject(subject) {
    Err(StorageError::AlreadyExists {
        entity: EntityKind::Subject,
        id,
    }) => {
        eprintln!("受试者 {id} 已存在");
        Ok(())
    }
    other => other,
}
# }
```

## 数据边界与运维建议

- 只向仓储传入通过 `domain` 构造函数创建、且之后未破坏其约束的对象；领域结构体多数具有公开字段，直接修改后调用方仍需自行保持一致性。
- 项目目录包含研究数据，应由应用配置合适的目录权限、磁盘加密、备份和访问审计；本 crate 不提供加密、身份认证或授权。
- 不要在项目打开且可能写入时复制单个内部文件作为备份。需要备份或迁移时，应在应用协调停止写入后复制完整项目目录，并通过重新打开及读取关键记录进行验证。
- `list_subjects` 和 `list_recording_metadata` 当前没有分页接口；数据量很大时，调用方需要评估内存和响应时间。
- 存储 API 为同步接口。桌面 UI 或异步服务应在后台线程或阻塞任务池中执行磁盘操作，避免阻塞事件循环。

## 测试

```bash
cargo test -p storage
cargo clippy -p storage --all-targets -- -D warnings
cargo fmt --all -- --check
```

`tests/project_storage.rs` 覆盖受试者 CRUD、重开持久化、显式错误、metadata/样本分离、记录更新、外键约束、受限删除及按受试者列举记录。
