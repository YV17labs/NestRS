import { Controller, Get } from '@nestjs/common';
import { HelloService } from './hello.service.js';

@Controller()
export class HelloController {
  constructor(private readonly helloService: HelloService) {}

  @Get('ping')
  ping(): string {
    return 'pong';
  }

  @Get('hello')
  hello(): { message: string } {
    return { message: this.helloService.greeting() };
  }
}
