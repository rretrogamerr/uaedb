# UAEDB Portable (Windows)

## 설치

1. GitHub 릴리즈에서 portable zip을 다운로드합니다.
2. zip을 원하는 폴더에 압축 해제합니다.
3. `runtime/` 폴더를 `uaedb.exe` 옆에 그대로 둡니다.

## 사용 방법

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta`는 반드시 파일이어야 합니다. 디렉터리는 오류가 납니다.
기본적으로 전체 uncompressed 번들에 패치를 적용합니다. 특정 엔트리에만
적용하려면 `--entry`를 사용하세요 (`--list-entries`로 전체 경로 목록을
확인할 수 있습니다).

일반 유저는 게임 폴더에서 `patch.bat`를 실행하면 됩니다. `data.unity3d`와
`data.xdelta`가 필요하며, `data.unity3d.bak`으로 백업한 뒤 성공 시
`data.unity3d`를 교체하고 백업은 유지합니다.

목록 확인 및 대상 엔트리 선택:

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d --list-entries
uaedb original.unity3d patch.xdelta original_patched.unity3d --entry "data.unity3d/GI/level84/..."
```

uncompressed 번들 기준 패치 생성:

```bash
uaedb original.unity3d --uncompress original.unity3d.uncompressed
uaedb modified.unity3d --uncompress modified.unity3d.uncompressed
xdelta3 -e -s original.unity3d.uncompressed modified.unity3d.uncompressed patch.xdelta
```

언컴프레스만 할 때:

```bash
uaedb original.unity3d --uncompress original.unity3d.uncompressed
```

UABEA의 `.decomp`와 동일한 형식의 uncompressed UnityFS 번들을 출력합니다.

## 참고

- `uaedb.exe`를 옮길 때는 `runtime/` 폴더도 함께 옮겨주세요.
- `xdelta3`를 찾지 못하면 `--xdelta`로 `xdelta3.exe`의 전체 경로를 지정하세요.
- `--keep-work` 옵션으로 보존된 작업 폴더 안의 `entry.bin`, `entry_patched.bin`,
  `bundle_patched.data` 또는 `bundle.uncompressed`, `bundle_patched.uncompressed`,
  `bundle.data`를 확인할 수 있습니다.
