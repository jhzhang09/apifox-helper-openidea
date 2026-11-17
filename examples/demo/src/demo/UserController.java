package demo;

import org.springframework.web.bind.annotation.*;
import org.springframework.http.ResponseEntity;
import java.util.List;

/**
 * 用户管理
 *
 * @author jhzhang
 * @since 2026-11-17
 */
@RestController
@RequestMapping("/api/users")
public class UserController {
    /**
     * 查询用户列表
     * @return 带地址与坐标的用户分页数据
     */
    @GetMapping
    public ResponseEntity<Result<Page<User>>> list() { return null; }

    /**
     * 创建用户
     * @param request 用户信息
     * @return 创建结果
     */
    @PostMapping
    public Result<User> create(@RequestBody User request) { return null; }

    /**
     * 查询单个用户
     * @param id 用户 ID
     * @return 用户详情
     */
    @GetMapping("/{id}")
    public Result<User> get(@PathVariable Long id) { return null; }
}
